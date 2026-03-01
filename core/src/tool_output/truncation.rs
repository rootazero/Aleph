//! Tool Output Truncation
//!
//! Provides smart truncation of tool outputs to prevent LLM context overflow.
//! Large outputs are saved to files, and a truncated preview with hints is returned.
//!
//! Inspired by OpenCode's tool/truncation.ts.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::Result;
use crate::utils::paths;

/// Maximum lines before truncation (default: 2000, matches OpenCode)
pub const MAX_LINES: usize = 2000;

/// Maximum bytes before truncation (default: 50KB, matches OpenCode)
pub const MAX_BYTES: usize = 50 * 1024;

/// Hint message when Task tool is available
pub const HINT_WITH_TASK: &str =
    "Full output saved to: {path}\nUse the Task tool to explore with Grep and Read.";

/// Hint message when Task tool is not available
pub const HINT_WITHOUT_TASK: &str =
    "Full output saved to: {path}\nUse Grep to search or Read with offset/limit.";

/// Counter for generating unique output file IDs
static OUTPUT_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Direction for truncation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TruncationDirection {
    /// Keep first N lines/bytes (head)
    #[default]
    Head,
    /// Keep last N lines/bytes (tail)
    Tail,
}

/// Configuration for truncation
#[derive(Debug, Clone)]
pub struct TruncationConfig {
    /// Maximum number of lines to keep
    pub max_lines: usize,
    /// Maximum number of bytes to keep
    pub max_bytes: usize,
    /// Direction for truncation
    pub direction: TruncationDirection,
    /// Whether agent has Task tool available (affects hint message)
    pub has_task_tool: bool,
}

impl Default for TruncationConfig {
    fn default() -> Self {
        Self {
            max_lines: MAX_LINES,
            max_bytes: MAX_BYTES,
            direction: TruncationDirection::Head,
            has_task_tool: true,
        }
    }
}

impl TruncationConfig {
    /// Create config with custom limits
    pub fn new(max_lines: usize, max_bytes: usize) -> Self {
        Self {
            max_lines,
            max_bytes,
            ..Default::default()
        }
    }

    /// Set truncation direction
    pub fn with_direction(mut self, direction: TruncationDirection) -> Self {
        self.direction = direction;
        self
    }

    /// Set whether Task tool is available
    pub fn with_task_tool(mut self, has_task_tool: bool) -> Self {
        self.has_task_tool = has_task_tool;
        self
    }
}

/// Result of truncation operation
#[derive(Debug, Clone)]
pub enum TruncationResult {
    /// Output was not truncated
    NotTruncated { content: String },
    /// Output was truncated
    Truncated(TruncatedOutput),
}

/// Details of a truncated output
#[derive(Debug, Clone)]
pub struct TruncatedOutput {
    /// Truncated content with hint
    pub content: String,
    /// Path where full output is saved
    pub output_path: PathBuf,
    /// Number of items removed (lines or bytes)
    pub removed_count: usize,
    /// Unit of removed items ("lines" or "bytes")
    pub unit: String,
    /// Total size before truncation
    pub original_size: usize,
}

impl TruncationResult {
    /// Get the content to send to LLM
    pub fn content(&self) -> &str {
        match self {
            TruncationResult::NotTruncated { content } => content,
            TruncationResult::Truncated(t) => &t.content,
        }
    }

    /// Check if output was truncated
    pub fn was_truncated(&self) -> bool {
        matches!(self, TruncationResult::Truncated(_))
    }

    /// Get output path if truncated
    pub fn output_path(&self) -> Option<&PathBuf> {
        match self {
            TruncationResult::NotTruncated { .. } => None,
            TruncationResult::Truncated(t) => Some(&t.output_path),
        }
    }
}

/// Truncate tool output if it exceeds limits
///
/// If the output exceeds the configured limits, it will be:
/// 1. Saved to a file in the tool-output directory
/// 2. Truncated according to the direction (head or tail)
/// 3. Returned with a hint about where the full output is saved
///
/// # Arguments
/// * `text` - The tool output text
/// * `config` - Truncation configuration
///
/// # Returns
/// * `TruncationResult` - Either the original content or truncated content with metadata
pub fn truncate_output(text: &str, config: &TruncationConfig) -> Result<TruncationResult> {
    let lines: Vec<&str> = text.lines().collect();
    let line_count = lines.len();
    let byte_count = text.len();

    // Check if truncation is needed
    if line_count <= config.max_lines && byte_count <= config.max_bytes {
        return Ok(TruncationResult::NotTruncated {
            content: text.to_string(),
        });
    }

    // Determine whether to truncate by lines or bytes
    let (truncated_content, removed_count, unit) = if line_count > config.max_lines {
        // Truncate by lines
        let kept_lines: Vec<&str> = match config.direction {
            TruncationDirection::Head => lines.iter().take(config.max_lines).copied().collect(),
            TruncationDirection::Tail => lines.iter().skip(line_count - config.max_lines).copied().collect(),
        };
        let removed = line_count - config.max_lines;
        (kept_lines.join("\n"), removed, "lines")
    } else {
        // Truncate by bytes
        let truncated = match config.direction {
            TruncationDirection::Head => {
                // Find a safe UTF-8 boundary
                let mut end = config.max_bytes;
                while end > 0 && !text.is_char_boundary(end) {
                    end -= 1;
                }
                text[..end].to_string()
            }
            TruncationDirection::Tail => {
                let mut start = byte_count - config.max_bytes;
                while start < byte_count && !text.is_char_boundary(start) {
                    start += 1;
                }
                text[start..].to_string()
            }
        };
        let removed = byte_count - truncated.len();
        (truncated, removed, "bytes")
    };

    // Save full output to file
    let output_path = save_full_output(text)?;

    // Build hint message
    let hint = if config.has_task_tool {
        HINT_WITH_TASK.replace("{path}", &output_path.display().to_string())
    } else {
        HINT_WITHOUT_TASK.replace("{path}", &output_path.display().to_string())
    };

    // Build truncation notice
    let direction_text = match config.direction {
        TruncationDirection::Head => "first",
        TruncationDirection::Tail => "last",
    };
    let notice = format!(
        "\n\n[... {} {} removed, showing {} {} ...]\n{}",
        removed_count,
        unit,
        direction_text,
        if unit == "lines" {
            format!("{} lines", config.max_lines)
        } else {
            format!("{} bytes", config.max_bytes)
        },
        hint
    );

    // Combine truncated content with notice
    let final_content = match config.direction {
        TruncationDirection::Head => format!("{}{}", truncated_content, notice),
        TruncationDirection::Tail => format!("{}{}", notice, truncated_content),
    };

    Ok(TruncationResult::Truncated(TruncatedOutput {
        content: final_content,
        output_path,
        removed_count,
        unit: unit.to_string(),
        original_size: if unit == "lines" { line_count } else { byte_count },
    }))
}

/// Save full output to a file
///
/// Files are saved with ascending IDs for easy identification and cleanup.
fn save_full_output(content: &str) -> Result<PathBuf> {
    let output_dir = get_tool_output_dir()?;

    // Ensure directory exists
    std::fs::create_dir_all(&output_dir)?;

    // Generate unique ID
    let id = OUTPUT_COUNTER.fetch_add(1, Ordering::SeqCst);
    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let filename = format!("tool_{}_{}.txt", timestamp, id);
    let path = output_dir.join(filename);

    // Write content
    std::fs::write(&path, content)?;

    tracing::debug!("Saved full tool output to: {}", path.display());
    Ok(path)
}

/// Get the tool output directory
pub fn get_tool_output_dir() -> Result<PathBuf> {
    paths::get_tool_output_dir()
}

#[cfg(test)]
mod tests {
    use super::*;
    

    #[test]
    fn test_no_truncation_needed() {
        let text = "Short output";
        let config = TruncationConfig::default();
        let result = truncate_output(text, &config).unwrap();

        assert!(!result.was_truncated());
        assert_eq!(result.content(), text);
    }

    #[test]
    fn test_truncation_by_lines_head() {
        let lines: Vec<String> = (0..3000).map(|i| format!("Line {}", i)).collect();
        let text = lines.join("\n");
        let config = TruncationConfig::new(100, usize::MAX).with_direction(TruncationDirection::Head);

        let result = truncate_output(&text, &config).unwrap();

        assert!(result.was_truncated());
        assert!(result.output_path().is_some());

        // Check that content starts with first lines
        let content = result.content();
        assert!(content.starts_with("Line 0\n"));
        assert!(content.contains("[... 2900 lines removed"));
    }

    #[test]
    fn test_truncation_by_lines_tail() {
        let lines: Vec<String> = (0..3000).map(|i| format!("Line {}", i)).collect();
        let text = lines.join("\n");
        let config = TruncationConfig::new(100, usize::MAX).with_direction(TruncationDirection::Tail);

        let result = truncate_output(&text, &config).unwrap();

        assert!(result.was_truncated());
        // Check that content ends with last lines
        let content = result.content();
        assert!(content.contains("Line 2999"));
        assert!(content.contains("[... 2900 lines removed"));
    }

    #[test]
    fn test_truncation_by_bytes_head() {
        let text = "x".repeat(100_000); // 100KB
        let config = TruncationConfig::new(usize::MAX, 1000).with_direction(TruncationDirection::Head);

        let result = truncate_output(&text, &config).unwrap();

        assert!(result.was_truncated());
        if let TruncationResult::Truncated(t) = &result {
            assert_eq!(t.unit, "bytes");
            assert!(t.removed_count > 0);
        }
    }

    #[test]
    fn test_hint_with_task_tool() {
        let lines: Vec<String> = (0..3000).map(|i| format!("Line {}", i)).collect();
        let text = lines.join("\n");
        let config = TruncationConfig::new(100, usize::MAX).with_task_tool(true);

        let result = truncate_output(&text, &config).unwrap();
        let content = result.content();

        assert!(content.contains("Task tool"));
    }

    #[test]
    fn test_hint_without_task_tool() {
        let lines: Vec<String> = (0..3000).map(|i| format!("Line {}", i)).collect();
        let text = lines.join("\n");
        let config = TruncationConfig::new(100, usize::MAX).with_task_tool(false);

        let result = truncate_output(&text, &config).unwrap();
        let content = result.content();

        assert!(content.contains("Grep to search"));
    }

    #[test]
    fn test_utf8_safe_truncation() {
        // Test that byte truncation respects UTF-8 boundaries
        let text = "你好世界".repeat(1000); // Chinese characters (3 bytes each)
        let config = TruncationConfig::new(usize::MAX, 100);

        let result = truncate_output(&text, &config).unwrap();

        assert!(result.was_truncated());
        // Content should still be valid UTF-8
        let content = result.content();
        assert!(content.is_char_boundary(0));
    }
}
