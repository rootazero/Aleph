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
use super::read_cache::{ReadCache, ReadCacheDecision};
use super::text::{clamp_line, is_binary, DEFAULT_READ_LINE_LIMIT, READ_WINDOW_TOKENS};
use crate::context::budget::pressure::chars_for_token_budget;
use crate::error::Result;
use crate::tools::AlephTool;

// =============================================================================
// Args & Output
// =============================================================================

/// Arguments for the `file_read` tool.
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

/// Output returned by the `file_read` tool.
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
    /// For image files: base64 (no data-URI prefix) of a model-ready copy of
    /// the image. Picked up by `hoist_inline_images` and re-emitted as a
    /// viewable `ContentBlock::Image`, so a vision model actually sees the file.
    /// `None` for every text/binary read (and skipped in the serialized shape).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_base64: Option<String>,
    /// Image format for [`Self::image_base64`] — `"png"` or `"jpeg"`. Drives the
    /// `format → MIME` map in the result pipeline. `None` for non-image reads.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
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
    /// Optional `ToolContext` handle for workspace-scoped output path resolution.
    tool_context_handle: Option<crate::tools::ToolContextHandle>,
    /// Cross-turn fingerprint store that collapses repeated reads of an
    /// unchanged file into a compact stub. Shared across `Clone`s of this tool.
    read_cache: ReadCache,
}

impl FileReadTool {
    /// Create a new `FileReadTool` with default settings.
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
            read_cache: ReadCache::default(),
        }
    }

    /// Configure the tool to use a `ToolContext` handle for workspace-scoped output paths.
    pub fn with_tool_context(mut self, handle: crate::tools::ToolContextHandle) -> Self {
        self.tool_context_handle = Some(handle);
        self
    }

    /// Resolve the output directory from the `ToolContext` handle (if available).
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
            // Share the fingerprint store so dedup spans the tool's lifetime,
            // not just a single registry clone.
            read_cache: self.read_cache.clone(),
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
            image_base64: None,
            format: None,
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
            image_base64: None,
            format: None,
        };
    }

    let line_end = start.saturating_add(limit).min(lines.len());
    // Pad to the full-file line count so numbers align across paginated reads
    // of the same file (matching `cat -n`), not just within this single window.
    let width = total_lines.to_string().len();

    // Second, independent limit: a character budget across the whole window.
    //
    // A line-only window is not a bound on context. 2000 lines × the 2000-char
    // `clamp_line` ceiling is up to 4 MB, and what happened next was worse than
    // just being big: `file_read` has no result budget of its own, so the
    // oversized window hit the generic head+tail truncator, which cut ~70/30 out
    // of the *middle* of the numbered content — while the `message`,
    // `returned_lines` and `truncated` fields minted here still described the
    // full window. The model was told "Showing lines 1-2000, pass offset=2001 to
    // continue" over content with a silent hole in it, and paging forward from
    // 2001 skipped the hole forever. Stopping at a char budget here keeps the
    // window and the message describing it consistent (pi applies the same
    // two-limits-whichever-first rule, `truncate.ts`).
    // Sized from the token budget, not from a fixed byte count: the same budget
    // buys ~2.5× more characters of source code than of CJK prose, so a flat
    // "50 KB per read" would blow the budget for one and waste it on the other.
    let char_budget = chars_for_token_budget(text, READ_WINDOW_TOKENS);

    let mut rendered = String::new();
    let mut chars = 0usize;
    let mut end = start;
    let mut stopped_on_chars = false;
    for (idx, line) in lines
        .get(start..line_end)
        .expect("invariant: start..line_end is clamped to lines length")
        .iter()
        .enumerate()
    {
        let clamped = clamp_line(line);
        let cost = clamped.chars().count() + width + 2; // gutter + tab + newline
                                                        // Always emit at least one line, so a single over-budget line still
                                                        // makes progress instead of returning an empty window forever.
        if chars + cost > char_budget && end > start {
            stopped_on_chars = true;
            break;
        }
        let lineno = start + idx + 1;
        rendered.push_str(&format!("{lineno:>width$}\t{clamped}\n"));
        chars += cost;
        end += 1;
    }

    let truncated = end < lines.len();
    let message = if truncated {
        let why = if stopped_on_chars {
            format!(" (~{READ_WINDOW_TOKENS}-token window limit)")
        } else {
            String::new()
        };
        format!(
            "Showing lines {}-{end} of {total_lines}{why}. Pass offset={} to continue.",
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
        image_base64: None,
        format: None,
    }
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
         line) and `limit` (max lines) to page through larger files. Image files \
         (png/jpeg/gif/tiff) are returned as a viewable image block so a vision \
         model can see them directly; other binary files are detected and reported \
         rather than dumped. The line-number prefixes are for reference only — \
         strip them before passing text to file_edit.";

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
            // A raster image the agent can actually *look at* (png/jpeg/gif/tiff)
            // is promoted to a viewable image block instead of a dead stub: the
            // `image_base64`/`format` fields below are lifted by
            // `hoist_inline_images` into a `ContentBlock::Image` for the model.
            // Non-image binaries fall through to the legacy "not displayable"
            // stub.
            if let Some(img) = super::image_read::encode_for_model(&bytes) {
                let message = format!(
                    "Image file — {}×{} px, {size} bytes, returned as a viewable image block.",
                    img.width, img.height
                );
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
                    image_base64: Some(img.base64),
                    format: Some(img.format.to_string()),
                });
            }
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
                image_base64: None,
                format: None,
            });
        }

        // Lossy decode: a few stray bytes degrade to U+FFFD rather than failing
        // the whole read.
        let text = String::from_utf8_lossy(&bytes);

        // Cross-turn unchanged-read guard: a cheap stat keyed against the last
        // read of this exact window. If the file is byte-for-byte unchanged,
        // skip re-rendering the (potentially large) numbered content and return
        // a compact stub instead. A failed stat fails open to a full read.
        let fingerprint = tokio::fs::metadata(&canonical)
            .await
            .ok()
            .and_then(|m| m.modified().ok())
            .map(|mtime| (mtime, size));
        let output = match self
            .read_cache
            .observe(&path_str, args.offset, args.limit, fingerprint)
        {
            ReadCacheDecision::Unchanged { repeats } => {
                unchanged_stub(&text, &args, size, path_str, repeats)
            }
            ReadCacheDecision::Fresh => render_window(&text, &args, size, path_str),
        };

        notify_tool_result(Self::NAME, &output.message, output.success);
        Ok(output)
    }
}

/// Compact response served on a repeat read of a byte-for-byte unchanged
/// window. The full numbered content is intentionally omitted — the model
/// received it on the first read — which is the whole point: it avoids
/// re-paying the token cost and breaks tight read loops. `total_lines` is still
/// reported (a cheap `lines().count()`) so the model keeps accurate file shape.
fn unchanged_stub(
    text: &str,
    args: &FileReadArgs,
    size: u64,
    path: String,
    repeats: u32,
) -> FileReadOutput {
    let total_lines = text.lines().count() as u64;
    let window = match (args.offset, args.limit) {
        (Some(o), Some(l)) => format!("lines {o}–{}", o.saturating_add(l)),
        (Some(o), None) => format!("from line {o}"),
        (None, Some(l)) => format!("first {l} lines"),
        (None, None) => "the whole file".to_string(),
    };
    let message = if repeats >= 2 {
        format!(
            "STOP — repeat #{repeats} of an identical read of an unchanged file \
             ({window}). The content has not changed since your first read and \
             is already in your context. Re-reading wastes tokens; act on what \
             you already have, or take a different step."
        )
    } else {
        format!(
            "Unchanged since your previous read ({window}): same modification \
             time and size, so the content is identical to what you already \
             received. Full content omitted to save context — re-read only after \
             the file changes."
        )
    };
    FileReadOutput {
        success: true,
        path,
        content: String::new(),
        size,
        total_lines,
        returned_lines: 0,
        truncated: false,
        message,
        image_base64: None,
        format: None,
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
    async fn image_file_is_returned_as_viewable_block() {
        use image::{DynamicImage, ImageFormat, RgbaImage};
        use std::io::Cursor;

        let dir = tempdir().unwrap();
        let file = dir.path().join("pixel.png");
        // A real PNG on disk — the read must promote it to an image payload.
        let mut png = Vec::new();
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255])))
            .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .unwrap();
        fs::write(&file, &png).unwrap();

        let out = AlephTool::call(&FileReadTool::new(), args(&file, None, None))
            .await
            .unwrap();
        assert!(out.success);
        // Pixels go out-of-band as a base64 image; the text channel stays empty.
        assert!(out.content.is_empty());
        assert_eq!(out.format.as_deref(), Some("png"));
        assert!(out.image_base64.is_some_and(|b| !b.is_empty()));
        assert!(out.message.contains("image block"), "msg: {}", out.message);
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
        use super::super::text::MAX_LINE_CHARS;
        let clamped = clamp_line(&"x".repeat(MAX_LINE_CHARS + 500));
        assert!(clamped.contains("line truncated"));
        assert!(clamped.starts_with(&"x".repeat(MAX_LINE_CHARS)));
    }

    /// The window has two limits and either can bind. When the char budget is
    /// the one that stops it, the reported window and the continuation offset
    /// must describe what was actually returned — the whole point is that
    /// `offset=N` picks up exactly where the content stopped, with no hole.
    #[test]
    fn window_stops_on_the_char_budget_and_reports_a_resumable_offset() {
        // 2000 lines × ~400 chars is well past the token window but inside the
        // 2000-line default limit, so only the char budget can stop it.
        let text: String = (0..2000)
            .map(|i| format!("line {i} {}\n", "payload ".repeat(50)))
            .collect();
        let args = FileReadArgs {
            path: String::new(),
            offset: None,
            limit: None,
        };
        let out = render_window(&text, &args, text.len() as u64, String::new());

        assert!(
            out.truncated,
            "an over-budget window must report truncation"
        );
        assert!(
            out.returned_lines < 2000,
            "the char budget must stop the window early, got {} lines",
            out.returned_lines
        );
        assert!(out.returned_lines > 0, "at least one line must be returned");
        assert!(
            out.message.contains("token window limit"),
            "the model must be told which limit bound; got: {}",
            out.message
        );

        // `returned_lines` and the advertised offset agree, and resuming there
        // returns the very next line — no silently skipped region.
        let next = out.returned_lines + 1;
        assert!(
            out.message
                .contains(&format!("Pass offset={next} to continue")),
            "continuation offset must match returned_lines; got: {}",
            out.message
        );
        let resumed = render_window(
            &text,
            &FileReadArgs {
                path: String::new(),
                offset: Some(next),
                limit: None,
            },
            text.len() as u64,
            String::new(),
        );
        let first_resumed = resumed
            .content
            .lines()
            .next()
            .expect("resumed window is non-empty");
        assert!(
            // The gutter is right-padded to the whole-file width, so compare the
            // trimmed number rather than the raw prefix.
            first_resumed.trim_start().starts_with(&format!("{next}\t")),
            "resuming must continue at the next line; got: {first_resumed}"
        );

        // And the window it returns fits the budget it promised, so the generic
        // truncator downstream never gets a chance to punch a hole in it.
        let tokens = crate::context::budget::pressure::estimate_tokens_smart(&out.content);
        assert!(
            tokens <= READ_WINDOW_TOKENS + 100,
            "window should land at/under its token budget, got {tokens}"
        );
    }

    /// A minified single-line file: the per-line clamp is what bounds it, and the
    /// window budget must not then swallow the rest of the file silently.
    #[test]
    fn minified_line_is_clamped_not_dropped() {
        let text = format!("{}\nsecond\n", "z".repeat(50_000));
        let args = FileReadArgs {
            path: String::new(),
            offset: None,
            limit: None,
        };
        let out = render_window(&text, &args, text.len() as u64, String::new());
        assert_eq!(
            out.returned_lines, 2,
            "both lines fit once line 1 is clamped"
        );
        assert!(!out.truncated, "nothing left to page to");
        assert!(
            out.content.contains("line truncated"),
            "the per-line clamp must announce itself: {}",
            &out.content[..out.content.len().min(80)]
        );
        assert!(out.content.contains("second"), "line 2 must survive");
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

    #[tokio::test]
    async fn repeat_read_of_unchanged_file_is_stubbed() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "first\nsecond\nthird\n").unwrap();

        // Same instance across both calls — the shared cache spans turns.
        let tool = FileReadTool::new();
        let first = AlephTool::call(&tool, args(&file, None, None))
            .await
            .unwrap();
        assert!(
            first.content.contains("1\tfirst"),
            "first read returns full content"
        );

        let second = AlephTool::call(&tool, args(&file, None, None))
            .await
            .unwrap();
        assert!(second.success);
        assert!(second.content.is_empty(), "repeat read omits content");
        assert_eq!(second.returned_lines, 0);
        assert_eq!(second.total_lines, 3, "file shape still reported");
        assert!(second.message.to_lowercase().contains("unchanged"));
    }

    #[tokio::test]
    async fn changed_file_returns_full_content_again() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "one\n").unwrap();

        let tool = FileReadTool::new();
        let _ = AlephTool::call(&tool, args(&file, None, None))
            .await
            .unwrap();

        // Mutating the file changes its size → the next read must be full again.
        fs::write(&file, "one\ntwo\nthree\n").unwrap();
        let again = AlephTool::call(&tool, args(&file, None, None))
            .await
            .unwrap();
        assert!(
            again.content.contains("3\tthree"),
            "post-change read is fresh"
        );
        assert_eq!(again.returned_lines, 3);
    }
}
