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
use super::text::{clamp_line, is_binary, read_window_tokens, DEFAULT_READ_LINE_LIMIT};
use crate::context::budget::pressure::{chars_for_result_token_budget, estimate_tokens_smart};
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

/// Default cap for `FileReadTool::max_read_size` — the previous
/// `100 * 1024 * 1024 // 100 MB` inline literal was duplicated in
/// three places (the default constructor, the `MAX_EDIT_BYTES` const
/// in `edit.rs`, and the 32 MB `FileWriteTool::call` cap), each
/// independent and prone to drift. Centralising the read cap here is
/// the first step; the write cap will follow when the
/// `FileWriteTool` constructor takes a `max_write_size` field.
const DEFAULT_MAX_READ_SIZE: u64 = 100 * 1024 * 1024;

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
            max_read_size: DEFAULT_MAX_READ_SIZE,
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

    // Second, independent limit: a character budget across the whole window. A
    // line-only window is not a bound on context — 2000 lines × the 2000-char
    // `clamp_line` ceiling is up to 4 MB (pi applies the same
    // two-limits-whichever-first rule, `truncate.ts`).
    //
    // This is a cheap *first cut* only, so a 2000-line file is not rendered in
    // full just to throw 1950 lines away; the feedback loop below is what actually
    // enforces the bound. It has to be, because this number is an open-loop guess
    // about a payload it never looks at: it charges the flattened result a flat
    // 2.5 chars/token on the grounds that `Value::to_string()` emits one line
    // containing `{`, but `content_ratio_with_baseline` charges every CJK
    // character at 1.5 *whatever* `looks_like_code` says — the code branch only
    // governs the non-CJK class. So the guess over-allocated a Chinese window by
    // ~1.22x (and, before it existed at all, prose and CSV by 1.4x).
    let char_budget = chars_for_result_token_budget(read_window_tokens());

    let mut rendered: Vec<String> = Vec::new();
    let mut chars = 0usize;
    let mut stopped_on_chars = false;
    for (idx, line) in lines.get(start..line_end).unwrap_or(&[]).iter().enumerate() {
        let clamped = clamp_line(line);
        let cost = clamped.chars().count() + width + 2; // gutter + tab + newline
                                                        // Always emit at least one line, so a single over-budget line still
                                                        // makes progress instead of returning an empty window forever.
        if chars + cost > char_budget && !rendered.is_empty() {
            stopped_on_chars = true;
            break;
        }
        let lineno = start + idx + 1;
        rendered.push(format!("{lineno:>width$}\t{clamped}\n"));
        chars += cost;
    }

    // Mint the window's self-description from the `kept` leading lines — the
    // invariant being that `message` / `returned_lines` / `truncated` and the
    // continuation offset describe exactly the lines that survive downstream, so
    // paging from the advertised offset never steps over a hole.
    let mint = |kept: usize, bounded: bool| -> FileReadOutput {
        let end = start + kept;
        let truncated = end < lines.len();
        let message = if truncated {
            let why = if bounded {
                format!(" (~{}-token window limit)", read_window_tokens())
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
            path: path.clone(),
            content: rendered.get(..kept).unwrap_or(&[]).concat(),
            size,
            total_lines,
            returned_lines: kept as u64,
            truncated,
            message,
            image_base64: None,
            format: None,
        }
    };

    // Close the loop: measure the window with the same estimator the Layer-2
    // backstop enforces, on the same string it enforces it against, and drop
    // trailing lines until it genuinely fits. `file_read` is deliberately never
    // persisted, so overrunning here means the generic head+tail truncator cuts
    // ~70/30 out of the MIDDLE of content whose fields were already minted for the
    // whole window — the model then pages forward from the advertised offset and
    // skips the removed region forever (and the result is no longer valid JSON).
    // A guess about the payload's ratio cannot promise that; a measurement can.
    let backstop = crate::tools::result_processing::read_backstop_tokens();
    let mut kept = rendered.len();
    let mut out = mint(kept, stopped_on_chars);
    while kept > 1 {
        // `FileReadOutput` only contains Strings, numbers, and bools, so
        // JSON serialization is effectively infallible. The previous
        // `.expect` would panic the worker on the (vanishingly rare)
        // off-chance a non-UTF-8 byte slipped into a field. Return
        // the original output unchanged so the caller at least sees
        // the data we managed to render.
        let flat = serde_json::to_string(&out).unwrap_or_else(|_| {
            tracing::warn!(
                "file_read: FileReadOutput JSON serialization failed mid-window; \
                 returning the un-serialized output (the worker is NOT panicking)"
            );
            String::new()
        });
        if flat.is_empty() {
            // Serialization truly failed — return what we have rather
            // than loop forever on `kept > 1` against an empty estimate.
            break;
        }
        let tokens = estimate_tokens_smart(&flat);
        if tokens <= backstop {
            break;
        }
        // Proportional shrink, floored one line below the current count so the
        // loop is strictly decreasing (and therefore terminates) even when the
        // per-line cost is wildly uneven.
        let target = (kept as f64 * backstop as f64 / tokens as f64 * 0.95) as usize;
        kept = target.clamp(1, kept - 1);
        out = mint(kept, true);
    }
    out
}

// =============================================================================
// AlephTool impl
// =============================================================================

#[async_trait]
impl AlephTool for FileReadTool {
    const NAME: &'static str = "file_read";
    const DESCRIPTION: &'static str =
        "Read a file's contents as `cat -n`-style numbered text lines. The window \
         is bounded two ways, whichever binds first: at most 2000 lines, and a \
         per-call size budget — so a wide or minified file returns fewer lines than \
         you asked for. The `message` always reports the exact range returned and, \
         when more remains, the `offset` to pass next; page with `offset` (1-based \
         starting line) and `limit` (max lines) rather than assuming you got the \
         whole window. Very long individual lines are clamped and say so. Image \
         files (png/jpeg/gif/tiff) are returned as a viewable image block so a \
         vision model can see them directly; other binary files are detected and \
         reported rather than dumped. Re-reading a window that has not changed \
         since your last read of it succeeds with an empty `content` — that means \
         the text you already received is still current, not that the file is \
         empty. The line-number prefixes are for reference only — strip them \
         before passing text to file_edit.";

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
    // Same clamping `render_window` applies, so the range named here is the range
    // the previous read actually covered rather than the raw arguments: `limit` is
    // a count, not a last line (`offset=2, limit=2` is lines 2-3, not 2-4), and it
    // cannot run past the file (`limit=900` on a 2-line file is lines 1-2).
    let start = args.offset.unwrap_or(1).max(1);
    let limit = args.limit.unwrap_or(DEFAULT_READ_LINE_LIMIT).max(1);
    let end = start
        .saturating_add(limit)
        .saturating_sub(1)
        .min(total_lines);
    let window = if start > total_lines {
        format!("offset {start}, past the end of the file")
    } else if start == 1 && end >= total_lines {
        "the whole file".to_string()
    } else {
        format!("lines {start}–{end}")
    };
    let message = if repeats >= 2 {
        format!(
            "STOP — repeat #{repeats} of an identical read of an unchanged file \
             ({window}). The content has not changed since your first read and \
             is already in your context; the content field is empty for that \
             reason, not because the file is. Re-reading wastes tokens; act on \
             what you already have, or take a different step."
        )
    } else {
        format!(
            "Unchanged since your previous read ({window}): same modification \
             time and size, so the content is identical to what you already \
             received. The content field is empty to save context, not because \
             the file is — re-read only after the file changes."
        )
    };
    FileReadOutput {
        success: true,
        path,
        content: String::new(),
        size,
        total_lines,
        returned_lines: 0,
        // Same meaning as on a real window: lines exist past the one described.
        // `false` here claimed the model held a complete view of a file it had
        // only been shown a slice of.
        truncated: end < total_lines,
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
        // Measured on the *flattened* result, because that is the string the
        // Layer-2 backstop is enforced against — measuring `content` alone is
        // what let the window overrun by 1.4x for prose while a test asserting
        // the raw-content figure still passed.
        let flat = serde_json::to_value(&out).unwrap().to_string();
        let tokens = crate::context::budget::pressure::estimate_tokens_smart(&flat);
        let backstop = crate::tools::result_processing::read_backstop_tokens();
        assert!(
            tokens < backstop,
            "the flattened window must fit the backstop it is sized against: \
             {tokens} tokens vs backstop {backstop}"
        );
    }

    /// The invariant the whole self-sizing exists for, asserted end to end and on
    /// the content type that used to break it.
    ///
    /// Prose scores 3.5 chars/token, but the *flattened* result is one line
    /// containing `{` and so always scores 2.5 — sizing the window in the file's
    /// own ratio space over-allocated by 1.4x, and `apply_result_budget` then cut
    /// the difference out of the MIDDLE while `message` / `returned_lines` /
    /// `truncated` still described the full window. Paging from the advertised
    /// offset skipped that hole forever.
    #[test]
    fn a_prose_window_survives_the_layer_two_backstop_intact() {
        // No code indicators anywhere, so the raw file scores the prose ratio.
        let text: String = (0..2000)
            .map(|i| format!("line {i} {}\n", "payload ".repeat(50)))
            .collect();
        let args = FileReadArgs {
            path: String::new(),
            offset: None,
            limit: None,
        };
        let out = render_window(
            &text,
            &args,
            text.len() as u64,
            "/tmp/prose.txt".to_string(),
        );
        let advertised = out.returned_lines;

        // Exactly what the dispatcher does: flatten, then apply Layer 2 with the
        // read-family budget (None).
        let flat = serde_json::to_value(&out).unwrap().to_string();
        let processed = crate::tools::result_processing::apply_result_budget(
            "c-prose",
            "file_read",
            &flat,
            None,
            None,
            None,
        );

        assert!(
            !processed.text.contains("[output truncated"),
            "the window must survive the backstop untouched; it was cut instead"
        );

        // Untouched means it is still valid JSON, so recover `content` and check
        // the gutters are contiguous 1..=advertised — no hole anywhere.
        let back: serde_json::Value =
            serde_json::from_str(&processed.text).expect("an untouched result is still valid JSON");
        let content = back["content"].as_str().expect("content survived");
        let seen: Vec<u64> = content
            .lines()
            .filter_map(|l| l.split('\t').next())
            .filter_map(|g| g.trim().parse::<u64>().ok())
            .collect();
        assert_eq!(
            seen.len() as u64,
            advertised,
            "every advertised line must be present, got {} of {advertised}",
            seen.len()
        );
        for (i, n) in seen.iter().enumerate() {
            assert_eq!(
                *n,
                i as u64 + 1,
                "line numbering must be contiguous: {seen:?}"
            );
        }
    }

    /// The same invariant on the content class the *sizing* was blind to.
    ///
    /// The flattened result always scores the code ratio for its Latin/symbol
    /// characters — but the estimator charges every CJK character at 1.5
    /// regardless, because `looks_like_code` only governs the non-CJK class. So a
    /// window sized at a flat 2.5 chars/token overran the backstop by ~1.7x on a
    /// Chinese file, `apply_result_budget` cut ~70/30 out of the MIDDLE, and the
    /// `message` / `returned_lines` / `truncated` fields minted for the whole
    /// window sent the model paging forward past the hole forever.
    #[test]
    fn a_cjk_window_survives_the_layer_two_backstop_intact() {
        // Overwhelmingly CJK, so the estimator charges it near the 1.5 ratio the
        // sizing never consulted.
        let text: String = (0..2000)
            .map(|i| format!("第{i}行：{}\n", "这是一段中文内容用来撑满这一行".repeat(4)))
            .collect();
        let args = FileReadArgs {
            path: String::new(),
            offset: None,
            limit: None,
        };
        let out = render_window(&text, &args, text.len() as u64, "/tmp/cjk.txt".to_string());
        let advertised = out.returned_lines;

        // Exactly what the dispatcher does: flatten, then apply Layer 2 with the
        // read-family budget (None).
        let flat = serde_json::to_value(&out).unwrap().to_string();
        let tokens = crate::context::budget::pressure::estimate_tokens_smart(&flat);
        let backstop = crate::tools::result_processing::read_backstop_tokens();
        assert!(
            tokens <= backstop,
            "the flattened CJK window must fit the backstop it is sized against: \
             {tokens} tokens vs backstop {backstop}"
        );
        let processed = crate::tools::result_processing::apply_result_budget(
            "c-cjk",
            "file_read",
            &flat,
            None,
            None,
            None,
        );

        assert!(
            !processed.text.contains("[output truncated"),
            "the CJK window must survive the backstop untouched; it was cut instead \
             ({} tokens vs backstop {})",
            crate::context::budget::pressure::estimate_tokens_smart(&flat),
            crate::tools::result_processing::read_backstop_tokens()
        );

        // Untouched means it is still valid JSON, so recover `content` and check
        // the gutters are contiguous 1..=advertised — no hole anywhere.
        let back: serde_json::Value =
            serde_json::from_str(&processed.text).expect("an untouched result is still valid JSON");
        let content = back["content"].as_str().expect("content survived");
        let seen: Vec<u64> = content
            .lines()
            .filter_map(|l| l.split('\t').next())
            .filter_map(|g| g.trim().parse::<u64>().ok())
            .collect();
        assert_eq!(
            seen.len() as u64,
            advertised,
            "every advertised line must be present, got {} of {advertised}",
            seen.len()
        );
        for (i, n) in seen.iter().enumerate() {
            assert_eq!(
                *n,
                i as u64 + 1,
                "line numbering must be contiguous: {seen:?}"
            );
        }

        // And resuming at the advertised offset returns the very next line.
        let next = advertised + 1;
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
            first_resumed.trim_start().starts_with(&format!("{next}\t")),
            "resuming must continue at the next line; got: {first_resumed}"
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

    /// The stub is still a `file_read` result, so its fields have to mean what
    /// the schema says they mean. Reporting `lines 2–4` for a `limit=2` read and
    /// `truncated: false` over empty content tells the model it holds the whole
    /// of a range it was never given.
    #[tokio::test]
    async fn unchanged_stub_describes_the_window_it_actually_covers() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "l1\nl2\nl3\nl4\nl5\n").unwrap();

        let tool = FileReadTool::new();
        let first = AlephTool::call(&tool, args(&file, Some(2), Some(2)))
            .await
            .unwrap();
        assert_eq!(first.returned_lines, 2, "first read covers lines 2-3");

        let stub = AlephTool::call(&tool, args(&file, Some(2), Some(2)))
            .await
            .unwrap();
        assert!(stub.content.is_empty(), "the stub omits content");
        assert!(
            stub.message.contains("lines 2–3"),
            "a limit=2 read from line 2 covers lines 2-3, not 2-4; got: {}",
            stub.message
        );
        assert!(
            stub.truncated,
            "lines 4-5 lie past the window, so this is not the whole file"
        );
        assert!(
            stub.message.contains("content field is empty"),
            "the model must be told what an empty content with success:true means; got: {}",
            stub.message
        );
    }

    /// A limit past EOF must not invent lines the file does not have.
    #[tokio::test]
    async fn unchanged_stub_clamps_its_window_to_the_file() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "one\ntwo\nthree\n").unwrap();

        let tool = FileReadTool::new();
        let _ = AlephTool::call(&tool, args(&file, Some(2), Some(900)))
            .await
            .unwrap();
        let stub = AlephTool::call(&tool, args(&file, Some(2), Some(900)))
            .await
            .unwrap();
        assert!(
            stub.message.contains("lines 2–3"),
            "the window must clamp to the file's 3 lines, not run to 901; got: {}",
            stub.message
        );
        assert!(
            !stub.truncated,
            "the previous read reached EOF, so nothing lies past it"
        );
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
