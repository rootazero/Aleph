//! `grep` — repository-aware content search.
//!
//! The tool that did not exist. Until it did, the only way for the model to
//! search file *contents* was `bash`, and `bash`'s own description has been
//! telling it since forever to "use `search` instead of `grep`" — where
//! `search` is the Tavily **web** search tool. The name resolved, so nothing
//! ever errored; the model just quietly got a web search or fell back to
//! `grep -r`, whose output ignores `.gitignore` and arrives unbounded.

use std::path::PathBuf;

use futures::stream::{self, StreamExt};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::notes;
use super::scan::{build_regex, render, scan_text, ScanOptions};
use super::walk::{display_path, walk, WalkRequest};
use crate::builtin_tools::error::ToolError;
use crate::builtin_tools::file_ops::{get_denied_paths, is_binary};
use crate::builtin_tools::{notify_tool_result, notify_tool_start};
use crate::error::Result;
use async_trait::async_trait;

use crate::tools::AlephTool;

/// Matches rendered when the caller names no `limit`.
const DEFAULT_LIMIT: usize = 60;
/// Ceiling on one page, so a bad `limit` cannot become the whole context.
const MAX_LIMIT: usize = 500;
/// Rendered matches kept for a single file before the rest are counted only.
const DEFAULT_MAX_PER_FILE: usize = 20;
/// Paths listed when `files_only` is set and the caller names no `limit`.
/// Higher than [`DEFAULT_LIMIT`] because a path costs a fraction of a match
/// block, and "which files even mention this" is the cheap first question.
const DEFAULT_FILES_ONLY_LIMIT: usize = 200;
/// Byte ceiling on the rendered block, applied at a line boundary.
const MAX_OUTPUT_BYTES: usize = 24 * 1024;
/// Files larger than this are counted as skipped rather than scanned. A source
/// file is never this big; a bundled asset or a checked-in dump is, and reading
/// one costs more than the answer it could contain.
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// Files read concurrently. Bounded so a wide repository cannot exhaust the
/// blocking pool that the rest of the turn's tools share.
const MAX_CONCURRENT_READS: usize = 16;
/// Ceiling on `context`.
///
/// Not tidiness — memory. A block is `2 * context + 1` lines and is built in
/// full *before* the byte cap trims anything, so an unbounded `context` would
/// let one call hold twenty near-complete copies of every matching file. The
/// clamp is reported when it binds; silently returning ten lines to a caller
/// who asked for fifty is the kind of quiet substitution this module's
/// messages exist to prevent.
const MAX_CONTEXT: usize = 10;

/// Arguments for the `grep` tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct GrepArgs {
    /// Regex to match. Use alternation (`foo|bar|baz`) to search several terms
    /// in one call.
    pub pattern: String,
    /// File or directory to search. Defaults to the workspace root.
    #[serde(default)]
    pub path: Option<String>,
    /// Filter files, e.g. `*.rs`, `src/**/*.rs`, `*.{rs,toml}`, `!*_test.rs`.
    #[serde(default)]
    pub glob: Option<String>,
    /// Case-insensitive match. Default false.
    #[serde(default)]
    pub ignore_case: Option<bool>,
    /// Treat `pattern` as a literal string. Default false.
    #[serde(default)]
    pub literal: Option<bool>,
    /// Lines of context around each match. Default 0.
    #[serde(default)]
    pub context: Option<u32>,
    /// Max matches to return. Default 60 (200 with `files_only`), max 500.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Skip this many matches — pass the `next_offset` from the last page.
    #[serde(default)]
    pub offset: Option<usize>,
    /// Return only the paths of files that contain a match. Default false.
    #[serde(default)]
    pub files_only: Option<bool>,
    /// Also search ignored and generated files. Default false.
    #[serde(default)]
    pub no_ignore: Option<bool>,
}

/// Result of a `grep` call.
#[derive(Debug, Clone, Serialize)]
pub struct GrepOutput {
    pub success: bool,
    /// The pattern as searched.
    pub pattern: String,
    /// Canonical root the paths below are relative to.
    pub root: String,
    /// `path:line: text` per match (`path-line- text` for context lines), or a
    /// newline-joined path list when `files_only` was set.
    pub matches: String,
    /// Matches (or paths) in this page.
    pub returned: usize,
    /// Matches in the whole search, not just this page.
    pub total_matches: usize,
    /// Files containing at least one match.
    pub files_with_matches: usize,
    /// Files actually read.
    pub files_scanned: usize,
    /// Some result was withheld — see `message` for which limit bound it.
    pub truncated: bool,
    /// `offset` for the next page, absent when this page is the last one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
    pub message: String,
}

/// Repository-aware content search.
#[derive(Clone)]
pub struct GrepTool {
    denied_paths: Vec<String>,
    tool_context_handle: Option<crate::tools::ToolContextHandle>,
}

impl GrepTool {
    #[must_use]
    pub fn new() -> Self {
        Self {
            denied_paths: get_denied_paths(),
            tool_context_handle: None,
        }
    }

    /// Use a `ToolContext` handle as the base for relative paths — the same
    /// base `file_read` uses, so `grep{path:"src"}` and a follow-up
    /// `file_read{path:"src/x.rs"}` cannot disagree about where `src` is.
    #[must_use]
    pub fn with_tool_context(mut self, handle: crate::tools::ToolContextHandle) -> Self {
        self.tool_context_handle = Some(handle);
        self
    }

    async fn output_dir(&self) -> Option<PathBuf> {
        match self.tool_context_handle {
            Some(ref handle) => Some(handle.read().await.output_dir.join("documents")),
            None => None,
        }
    }
}

impl Default for GrepTool {
    fn default() -> Self {
        Self::new()
    }
}

/// One file's contribution, in walk order.
struct FileOutcome {
    path: PathBuf,
    total: usize,
    /// Rendered lines per match, already prefixed with the display path.
    blocks: Vec<Vec<String>>,
    /// Read failed, or the file was binary / over [`MAX_FILE_BYTES`].
    skipped: bool,
}

impl GrepTool {
    async fn run(&self, args: GrepArgs) -> std::result::Result<GrepOutput, ToolError> {
        let files_only = args.files_only.unwrap_or(false);
        let limit = args
            .limit
            .unwrap_or(if files_only {
                DEFAULT_FILES_ONLY_LIMIT
            } else {
                DEFAULT_LIMIT
            })
            .clamp(1, MAX_LIMIT);
        let offset = args.offset.unwrap_or(0);
        let re = build_regex(
            &args.pattern,
            args.literal.unwrap_or(false),
            args.ignore_case.unwrap_or(false),
        )
        .map_err(|e| ToolError::InvalidArgs(format!("Invalid pattern '{}': {e}", args.pattern)))?;

        let output_dir = self.output_dir().await;
        let path = args.path.as_deref().unwrap_or(".");
        let (root, mut report) = walk(&WalkRequest {
            path,
            glob: args.glob.as_deref(),
            respect_ignore: !args.no_ignore.unwrap_or(false),
            denied_paths: &self.denied_paths,
            output_dir: output_dir.as_deref(),
        })?;

        // Rendering budget: enough to serve this page and prove whether another
        // exists, never the whole tree. Totals below are still exact — the
        // budget bounds memory, not the count.
        let render_budget = offset.saturating_add(limit).saturating_add(1);
        let requested_context = args.context.unwrap_or(0) as usize;
        let context = requested_context.min(MAX_CONTEXT);
        let scan_opts = ScanOptions {
            context,
            max_per_file: if files_only { 1 } else { DEFAULT_MAX_PER_FILE },
        };

        let root_for_scan = root.clone();
        let files = std::mem::take(&mut report.files);
        let outcomes: Vec<FileOutcome> = stream::iter(files)
            .map(|file| {
                let re = re.clone();
                let root = root_for_scan.clone();
                let context = scan_opts.context;
                let max_per_file = scan_opts.max_per_file;
                async move {
                    tokio::task::spawn_blocking(move || {
                        scan_one(&file, &root, &re, context, max_per_file)
                    })
                    .await
                    .unwrap_or_else(|_| FileOutcome {
                        path: PathBuf::new(),
                        total: 0,
                        blocks: Vec::new(),
                        skipped: true,
                    })
                }
            })
            // Ordered: `buffered` yields in input order, so the page a given
            // `offset` lands on is the same page on every call. `buffer_unordered`
            // would make paging non-deterministic for a free speedup nobody asked
            // for.
            .buffered(MAX_CONCURRENT_READS)
            .collect()
            .await;

        let mut total_matches = 0usize;
        let mut files_with_matches = 0usize;
        let mut files_scanned = 0usize;
        let mut skipped_files = 0usize;
        let mut pool: Vec<Vec<String>> = Vec::new();
        let mut file_paths: Vec<String> = Vec::new();
        let mut per_file_capped = false;

        for outcome in outcomes {
            if outcome.skipped {
                skipped_files += 1;
                continue;
            }
            files_scanned += 1;
            if outcome.total == 0 {
                continue;
            }
            total_matches += outcome.total;
            files_with_matches += 1;
            per_file_capped |= !files_only && outcome.total > outcome.blocks.len();
            if files_only {
                if file_paths.len() < render_budget {
                    file_paths.push(display_path(&outcome.path, &root));
                }
            } else if pool.len() < render_budget {
                pool.extend(outcome.blocks);
            }
        }

        // In `files_only` the page is over files; otherwise it is over matches.
        let (universe, page_source): (usize, Vec<Vec<String>>) = if files_only {
            (
                files_with_matches,
                file_paths.into_iter().map(|p| vec![p]).collect(),
            )
        } else {
            (total_matches, pool)
        };

        // The page is measured in BLOCKS — one block is one match (or one path
        // under `files_only`) — and the byte cap is applied to whole blocks for
        // the same reason. Cutting the flattened lines instead would make
        // `returned` and `next_offset` describe matches the caller never saw,
        // so the next page would start past them: a window that silently skips
        // what it withheld.
        let blocks: Vec<Vec<String>> = page_source.into_iter().skip(offset).take(limit).collect();
        let (rendered, returned, byte_capped) = cap_bytes(&blocks);
        let next_offset =
            (offset.saturating_add(returned) < universe).then(|| offset.saturating_add(returned));

        let truncated =
            byte_capped || next_offset.is_some() || report.walk_capped || per_file_capped;

        // Say what was found AND what was withheld, naming the lever each
        // time. The clauses that `find` owes too come from `notes`, so an
        // omission cannot be spelled one way here and another way there.
        let respected_ignore = !args.no_ignore.unwrap_or(false);
        let mut message = if universe == 0 {
            let mut msg = format!("No matches in {files_scanned} file(s) searched");
            msg.push_str(&notes::ignored(&report, respected_ignore).unwrap_or_default());
            msg
        } else {
            let unit = if files_only { "file" } else { "match" };
            let mut msg = format!(
                "{returned} of {universe} {unit}(es) across {files_with_matches} file(s); \
                 {files_scanned} file(s) searched"
            );
            if let Some(next) = next_offset {
                msg.push_str(&notes::paging(next, limit));
            }
            if byte_capped {
                msg.push_str(". Output hit its byte cap; narrow the pattern or add a glob");
            }
            if per_file_capped {
                msg.push_str(&format!(
                    ". At least one file had more than {DEFAULT_MAX_PER_FILE} matches; only that \
                     many are rendered per file, though the totals above count them all"
                ));
            }
            if requested_context > context {
                msg.push_str(&format!(
                    ". context was clamped from {requested_context} to {MAX_CONTEXT} lines"
                ));
            }
            msg.push_str(&notes::walk_capped(&report, "glob").unwrap_or_default());
            msg
        };
        if skipped_files > 0 {
            message.push_str(&format!(
                ". {skipped_files} binary or oversized file(s) not searched"
            ));
        }
        message.push_str(&notes::withheld(report.denied).unwrap_or_default());
        message.push('.');

        Ok(GrepOutput {
            success: true,
            pattern: args.pattern,
            root: root.to_string_lossy().to_string(),
            matches: rendered,
            returned,
            total_matches,
            files_with_matches,
            files_scanned,
            truncated,
            next_offset,
            message,
        })
    }
}

fn scan_one(
    file: &PathBuf,
    root: &std::path::Path,
    re: &regex::Regex,
    context: usize,
    max_per_file: usize,
) -> FileOutcome {
    let skipped = FileOutcome {
        path: file.clone(),
        total: 0,
        blocks: Vec::new(),
        skipped: true,
    };
    match std::fs::metadata(file) {
        Ok(meta) if meta.len() > MAX_FILE_BYTES => return skipped,
        Ok(_) => {}
        Err(_) => return skipped,
    }
    let Ok(bytes) = std::fs::read(file) else {
        return skipped;
    };
    if is_binary(&bytes) {
        return skipped;
    }
    let text = String::from_utf8_lossy(&bytes);
    let scan = scan_text(
        &text,
        re,
        &ScanOptions {
            context,
            max_per_file,
        },
    );
    if scan.is_empty() {
        return FileOutcome {
            path: file.clone(),
            total: 0,
            blocks: Vec::new(),
            skipped: false,
        };
    }
    let display = display_path(file, root);
    let blocks = scan.rendered.iter().map(|m| render(&display, m)).collect();
    FileOutcome {
        path: file.clone(),
        total: scan.total,
        blocks,
        skipped: false,
    }
}

/// Render whole blocks until [`MAX_OUTPUT_BYTES`] would be exceeded.
///
/// Returns `(text, blocks_rendered, capped)`. Blocks, not lines: the count it
/// returns is what `returned` and `next_offset` are built from, so a partially
/// rendered match would hand the caller a cursor past bytes it never received.
///
/// A single block larger than the whole budget is emitted anyway. Refusing it
/// would return an empty page with a `next_offset` that has not advanced —
/// a caller paging politely would loop forever on it.
fn cap_bytes(blocks: &[Vec<String>]) -> (String, usize, bool) {
    let mut out = String::new();
    for (i, block) in blocks.iter().enumerate() {
        let size: usize = block.iter().map(|l| l.len() + 1).sum();
        if i > 0 && out.len() + size > MAX_OUTPUT_BYTES {
            return (out, i, true);
        }
        for line in block {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(line);
        }
    }
    (out, blocks.len(), false)
}

#[async_trait]
impl AlephTool for GrepTool {
    const NAME: &'static str = "grep";

    const DESCRIPTION: &'static str = r#"Search file CONTENTS across a tree. Use this instead of `bash` with grep/rg/ack — it obeys .gitignore, skips binaries and `.git`, and returns a bounded, pageable result. A `bash` grep does none of that: one recursive run pours every hit under node_modules/, target/ and dist/ into the context window.

`pattern` is a regex, so several terms are ONE call: `pattern: "TokenBudget|CacheMonitor|EXEC_WORKSPACE"`. Never issue one call per term. Set `literal: true` to search text containing regex metacharacters.

Returns `path:line: text` (context lines as `path-line- text`) relative to `root`. Match lines are clamped to 240 chars — they are locators, not content. Once you have the line number, read the neighbourhood with `file_read{path, offset, limit}` rather than reading whole files.

Start wide and cheap with `files_only: true` (paths only, no lines) when the question is "where does this live"; drop it once the file set is small.

Bounds and levers, all reported in `message`: `limit` (default 60 matches, 200 paths, max 500) with `offset` for the next page — `next_offset` comes back when there is one; at most 20 rendered matches per file though `total_matches` counts them all; `glob` narrows files (`*.rs`, `src/**/*.rs`, `*.{rs,toml}`, `!*_test.rs`); `context` adds surrounding lines; `no_ignore: true` also searches ignored/generated files.

Protected locations (credential dirs, the operator's deny_read_globs) are withheld and counted, never silently dropped."#;

    type Args = GrepArgs;
    type Output = GrepOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        notify_tool_start(Self::NAME, &args.pattern);
        let out = self
            .run(args)
            .await
            .inspect_err(|e| notify_tool_result(Self::NAME, &e.to_string(), false))?;
        notify_tool_result(Self::NAME, &out.message, true);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn fixture() -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".gitignore"), "vendor/\n").unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("src/a.rs"),
            "fn alpha() {}\nlet needle = 1;\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("src/b.rs"),
            "// needle here\nfn beta() {}\n",
        )
        .unwrap();
        fs::write(dir.path().join("notes.md"), "needle in markdown\n").unwrap();
        fs::create_dir(dir.path().join("vendor")).unwrap();
        fs::write(dir.path().join("vendor/dep.rs"), "needle needle needle\n").unwrap();
        dir
    }

    fn args(dir: &TempDir, pattern: &str) -> GrepArgs {
        GrepArgs {
            pattern: pattern.to_string(),
            path: Some(dir.path().to_string_lossy().to_string()),
            glob: None,
            ignore_case: None,
            literal: None,
            context: None,
            limit: None,
            offset: None,
            files_only: None,
            no_ignore: None,
        }
    }

    #[tokio::test]
    async fn gitignored_matches_never_reach_the_context() {
        let dir = fixture();
        let out = GrepTool::new().run(args(&dir, "needle")).await.unwrap();
        assert_eq!(out.total_matches, 3, "{}", out.matches);
        assert!(!out.matches.contains("vendor/"), "{}", out.matches);
        assert!(
            out.matches.contains("src/a.rs:2: let needle = 1;"),
            "{}",
            out.matches
        );
    }

    #[tokio::test]
    async fn no_ignore_reaches_the_ignored_tree() {
        let dir = fixture();
        let mut a = args(&dir, "needle");
        a.no_ignore = Some(true);
        let out = GrepTool::new().run(a).await.unwrap();
        assert!(out.matches.contains("vendor/dep.rs"), "{}", out.matches);
    }

    #[tokio::test]
    async fn glob_narrows_to_one_language() {
        let dir = fixture();
        let mut a = args(&dir, "needle");
        a.glob = Some("*.md".into());
        let out = GrepTool::new().run(a).await.unwrap();
        assert_eq!(out.files_with_matches, 1);
        assert!(out.matches.starts_with("notes.md:1:"), "{}", out.matches);
    }

    #[tokio::test]
    async fn files_only_returns_paths_not_lines() {
        let dir = fixture();
        let mut a = args(&dir, "needle");
        a.files_only = Some(true);
        let out = GrepTool::new().run(a).await.unwrap();
        assert_eq!(out.returned, 3);
        assert!(!out.matches.contains("let needle"), "{}", out.matches);
        for line in out.matches.lines() {
            assert!(!line.contains(':'), "{line}");
        }
    }

    /// The pageable half: `offset` must land on the same sequence every time,
    /// and the pages must partition the result rather than overlap it.
    #[tokio::test]
    async fn pages_partition_the_result_deterministically() {
        let dir = fixture();
        let mut first = args(&dir, "needle");
        first.limit = Some(2);
        let page1 = GrepTool::new().run(first).await.unwrap();
        assert_eq!(page1.returned, 2);
        assert_eq!(page1.next_offset, Some(2));

        let mut second = args(&dir, "needle");
        second.limit = Some(2);
        second.offset = page1.next_offset;
        let page2 = GrepTool::new().run(second).await.unwrap();
        assert_eq!(page2.returned, 1);
        assert_eq!(page2.next_offset, None);

        let all: Vec<&str> = page1.matches.lines().chain(page2.matches.lines()).collect();
        assert_eq!(all.len(), 3);
        let mut deduped = all.clone();
        deduped.sort_unstable();
        deduped.dedup();
        assert_eq!(deduped.len(), 3, "pages overlapped: {all:?}");
    }

    /// Alternation is the answer to "several terms" — the reason there is no
    /// separate multi-pattern verb.
    #[tokio::test]
    async fn alternation_answers_several_terms_in_one_call() {
        let dir = fixture();
        let out = GrepTool::new()
            .run(args(&dir, "alpha|beta|nowhere"))
            .await
            .unwrap();
        assert_eq!(out.total_matches, 2);
        assert_eq!(out.files_with_matches, 2);
    }

    #[tokio::test]
    async fn a_binary_file_is_skipped_and_counted_not_dumped() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("blob.bin"), b"needle\x00\x00binary").unwrap();
        fs::write(dir.path().join("ok.txt"), "needle\n").unwrap();
        let out = GrepTool::new().run(args(&dir, "needle")).await.unwrap();
        assert_eq!(out.total_matches, 1);
        assert!(
            out.message.contains("binary or oversized"),
            "{}",
            out.message
        );
    }

    /// `returned` counts matches, not rendered lines — with `context` on, one
    /// match is three lines, and a count that drifted to lines would not be
    /// comparable to `total_matches` sitting beside it.
    #[tokio::test]
    async fn returned_counts_matches_even_when_context_multiplies_the_lines() {
        let dir = fixture();
        let mut a = args(&dir, "needle");
        a.context = Some(1);
        let out = GrepTool::new().run(a).await.unwrap();
        assert_eq!(out.returned, 3);
        assert_eq!(out.total_matches, 3);
        assert!(out.matches.lines().count() > 3, "{}", out.matches);
    }

    /// A clamp the caller is not told about is a quiet substitution; this one
    /// says so in the same `message` that names every other omission.
    #[tokio::test]
    async fn an_oversized_context_is_clamped_out_loud() {
        let dir = fixture();
        let mut a = args(&dir, "needle");
        a.context = Some(500);
        let out = GrepTool::new().run(a).await.unwrap();
        assert!(
            out.message.contains("clamped from 500 to 10"),
            "{}",
            out.message
        );
    }

    #[tokio::test]
    async fn context_lines_come_back_with_the_dash_convention() {
        let dir = fixture();
        let mut a = args(&dir, "needle");
        a.glob = Some("*.rs".into());
        a.context = Some(1);
        let out = GrepTool::new().run(a).await.unwrap();
        assert!(
            out.matches.contains("src/a.rs-1- fn alpha() {}"),
            "{}",
            out.matches
        );
    }

    #[tokio::test]
    async fn an_empty_result_says_what_it_searched() {
        let dir = fixture();
        let out = GrepTool::new()
            .run(args(&dir, "zzz-not-here"))
            .await
            .unwrap();
        assert_eq!(out.total_matches, 0);
        assert!(out.message.starts_with("No matches in"), "{}", out.message);
        assert!(out.message.contains("file(s) searched"), "{}", out.message);
    }

    /// The description quotes five numbers that are constants at the top of
    /// this file. Two copies of one fact drift, and the copy the model reads is
    /// the one that changes its behaviour: a description promising `limit` up
    /// to 500 against a clamp of 200 costs a call to discover the truth, and a
    /// promised 240-char clamp against a 2 000-char one silently reintroduces
    /// the flood this tool exists to stop.
    #[test]
    fn the_description_quotes_the_limits_it_actually_enforces() {
        let description = <GrepTool as AlephTool>::DESCRIPTION;
        for (what, value) in [
            ("default match limit", DEFAULT_LIMIT),
            ("files_only default limit", DEFAULT_FILES_ONLY_LIMIT),
            ("max limit", MAX_LIMIT),
            ("per-file render cap", DEFAULT_MAX_PER_FILE),
            ("match-line clamp", super::super::scan::MATCH_LINE_CHARS),
        ] {
            assert!(
                description.contains(&value.to_string()),
                "DESCRIPTION never states the {what} ({value}); the model is reading a bound \
                 this code does not enforce"
            );
        }
    }

    #[tokio::test]
    async fn an_invalid_regex_is_rejected_by_name() {
        let dir = fixture();
        let err = GrepTool::new().run(args(&dir, "a(")).await.unwrap_err();
        assert!(err.to_string().contains("Invalid pattern"), "{err}");
    }

    #[tokio::test]
    async fn a_protected_location_is_reported_not_silently_dropped() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("creds")).unwrap();
        fs::write(dir.path().join("creds/id_rsa"), "needle").unwrap();
        fs::write(dir.path().join("ok.txt"), "needle").unwrap();

        let canonical = dir.path().canonicalize().unwrap();
        let tool = GrepTool {
            denied_paths: vec![canonical.join("creds").to_string_lossy().to_string()],
            tool_context_handle: None,
        };
        let out = tool.run(args(&dir, "needle")).await.unwrap();
        assert_eq!(out.total_matches, 1);
        assert!(
            out.message.contains("protected-location"),
            "{}",
            out.message
        );
    }

    #[test]
    fn byte_cap_cuts_at_a_block_boundary_and_reports_the_count_it_rendered() {
        let blocks: Vec<Vec<String>> = (0..100)
            .map(|_| vec!["x".repeat(1000), "y".repeat(1000)])
            .collect();
        let (out, rendered, capped) = cap_bytes(&blocks);
        assert!(capped);
        assert!(out.len() <= MAX_OUTPUT_BYTES);
        // Whole blocks only: an odd line count would mean a match was rendered
        // half-way and the cursor would step past its missing half.
        assert_eq!(out.lines().count(), rendered * 2);
        assert!(rendered < blocks.len());
    }

    /// A block that cannot fit is still emitted: an empty page whose
    /// `next_offset` has not advanced is a paging loop.
    #[test]
    fn a_single_oversized_block_is_emitted_rather_than_stalling_the_cursor() {
        let blocks = vec![vec!["z".repeat(MAX_OUTPUT_BYTES * 2)]];
        let (out, rendered, capped) = cap_bytes(&blocks);
        assert_eq!(rendered, 1);
        assert!(!capped);
        assert!(!out.is_empty());
    }
}
