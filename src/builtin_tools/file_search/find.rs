//! `find` — repository-aware file discovery by glob.
//!
//! The name half of the pair `grep` completes. Inside this module the rule
//! holds without exception: `grep` and `find` do not each answer "which files
//! exist" — [`super::walk`] answers it once, for both.
//!
//! # The residual duplication, stated rather than hidden
//!
//! `file_ops{operation:"search"}` also globs for files, and it does **not** go
//! through [`super::walk`]. It keeps its own `glob` crate walk and its own
//! `SKIPPED_DIRS` floor, so this repository does carry two implementations of
//! "list files matching a pattern". They were left apart on purpose, and the
//! purpose is worth writing down because the next reader will otherwise merge
//! them:
//!
//! - they answer different questions — that one is the **file-management**
//!   face (returns `FileInfo` with size/type/extension, feeds `organize` /
//!   `batch_move` / `stats` over arbitrary directories such as a downloads
//!   folder, where `.gitignore` is not a meaningful filter), this one is the
//!   **code-navigation** face (`.gitignore`-aware, paths only, pageable);
//! - migrating `file_ops` onto this walker would change what it returns on any
//!   directory that happens to sit in a repository, and its `SKIPPED_DIRS`
//!   helper is shared with `stats`, so the change lands on three operations
//!   rather than one.
//!
//! That is a trade, not a clean result: if the two ever start disagreeing about
//! something a caller can observe, the answer is to move `file_ops` onto this
//! walker, not to teach it a second set of rules.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::notes;
use super::walk::{display_path, walk, WalkRequest};
use crate::builtin_tools::error::ToolError;
use crate::builtin_tools::file_ops::get_denied_paths;
use crate::builtin_tools::{notify_tool_result, notify_tool_start};
use crate::error::Result;
use async_trait::async_trait;

use crate::tools::AlephTool;

/// Paths returned when the caller names no `limit`.
const DEFAULT_LIMIT: usize = 200;
/// Ceiling on one page.
const MAX_LIMIT: usize = 2000;

/// Arguments for the `find` tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct FindArgs {
    /// Glob to match, e.g. `*.rs`, `src/**/*.rs`, `*.{rs,toml}`, `!*_test.rs`.
    pub pattern: String,
    /// Directory to search. Defaults to the workspace root.
    #[serde(default)]
    pub path: Option<String>,
    /// Max paths to return. Default 200, max 2000.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Skip this many paths — pass the `next_offset` from the last page.
    #[serde(default)]
    pub offset: Option<usize>,
    /// Also list ignored and generated files. Default false.
    #[serde(default)]
    pub no_ignore: Option<bool>,
}

/// Result of a `find` call.
#[derive(Debug, Clone, Serialize)]
pub struct FindOutput {
    pub success: bool,
    /// Canonical root the paths below are relative to.
    pub root: String,
    /// Newline-joined paths, sorted, relative to `root`.
    pub paths: String,
    /// Paths in this page.
    pub returned: usize,
    /// Paths matching the glob in the whole tree, not just this page.
    pub total: usize,
    /// Some result was withheld — see `message`.
    pub truncated: bool,
    /// `offset` for the next page, absent when this page is the last one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
    pub message: String,
}

/// Repository-aware file discovery.
#[derive(Clone)]
pub struct FindTool {
    denied_paths: Vec<String>,
    tool_context_handle: Option<crate::tools::ToolContextHandle>,
}

impl FindTool {
    #[must_use]
    pub fn new() -> Self {
        Self {
            denied_paths: get_denied_paths(),
            tool_context_handle: None,
        }
    }

    /// Use a `ToolContext` handle as the base for relative paths — the same
    /// base `file_read` and `grep` use.
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

    async fn run(&self, args: FindArgs) -> std::result::Result<FindOutput, ToolError> {
        let limit = args.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        let offset = args.offset.unwrap_or(0);
        let output_dir = self.output_dir().await;
        let path = args.path.as_deref().unwrap_or(".");

        let (root, report) = walk(&WalkRequest {
            path,
            glob: Some(args.pattern.as_str()),
            respect_ignore: !args.no_ignore.unwrap_or(false),
            denied_paths: &self.denied_paths,
            output_dir: output_dir.as_deref(),
        })?;

        let total = report.files.len();
        let page: Vec<String> = report
            .files
            .iter()
            .skip(offset)
            .take(limit)
            .map(|p| display_path(p, &root))
            .collect();
        let returned = page.len();
        let next_offset =
            (offset.saturating_add(returned) < total).then(|| offset.saturating_add(returned));

        let respected_ignore = !args.no_ignore.unwrap_or(false);
        let mut message = if total == 0 {
            let mut msg = String::from("No files matched");
            msg.push_str(&notes::ignored(&report, respected_ignore).unwrap_or_default());
            msg
        } else {
            let mut msg = format!("{returned} of {total} file(s)");
            if let Some(next) = next_offset {
                msg.push_str(&notes::paging(next, limit));
            }
            msg.push_str(&notes::walk_capped(&report, "pattern").unwrap_or_default());
            msg
        };
        message.push_str(&notes::withheld(report.denied).unwrap_or_default());
        message.push('.');

        Ok(FindOutput {
            success: true,
            root: root.to_string_lossy().to_string(),
            paths: page.join("\n"),
            returned,
            total,
            truncated: next_offset.is_some() || report.walk_capped,
            next_offset,
            message,
        })
    }
}

impl Default for FindTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AlephTool for FindTool {
    const NAME: &'static str = "find";

    const DESCRIPTION: &'static str = r#"Find files by glob across a tree. Use this instead of `bash` with find/ls -R — it obeys .gitignore, never descends into `.git`, and returns a bounded, pageable, sorted path list.

`pattern` is a glob matched at any depth: `*.rs`, `src/**/*.rs`, `*.{rs,toml}`, `!*_test.rs` (leading `!` excludes). Pass `*` to list everything under `path`.

Returns paths relative to `root`, newline-joined. `limit` defaults to 200 (max 2000) with `offset` for the next page; `next_offset` comes back when there is one. `no_ignore: true` also lists ignored and generated files. Ignored directories, protected locations and a hit walk cap are each reported in `message` rather than silently dropped.

For contents rather than names use `grep`. For file sizes/types, or to move/copy/organize what you found, use `file_ops`."#;

    type Args = FindArgs;
    type Output = FindOutput;

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
        fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
        fs::create_dir_all(dir.path().join("src/deep")).unwrap();
        fs::write(dir.path().join("src/a.rs"), "").unwrap();
        fs::write(dir.path().join("src/deep/b.rs"), "").unwrap();
        fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        fs::create_dir(dir.path().join("target")).unwrap();
        fs::write(dir.path().join("target/gen.rs"), "").unwrap();
        dir
    }

    fn args(dir: &TempDir, pattern: &str) -> FindArgs {
        FindArgs {
            pattern: pattern.to_string(),
            path: Some(dir.path().to_string_lossy().to_string()),
            limit: None,
            offset: None,
            no_ignore: None,
        }
    }

    #[tokio::test]
    async fn gitignored_files_never_reach_the_listing() {
        let dir = fixture();
        let out = FindTool::new().run(args(&dir, "*.rs")).await.unwrap();
        assert_eq!(out.total, 2, "{}", out.paths);
        assert!(!out.paths.contains("target/"), "{}", out.paths);
    }

    #[tokio::test]
    async fn no_ignore_lists_the_generated_tree() {
        let dir = fixture();
        let mut a = args(&dir, "*.rs");
        a.no_ignore = Some(true);
        let out = FindTool::new().run(a).await.unwrap();
        assert!(out.paths.contains("target/gen.rs"), "{}", out.paths);
    }

    #[tokio::test]
    async fn star_lists_everything_tracked() {
        let dir = fixture();
        let out = FindTool::new().run(args(&dir, "*")).await.unwrap();
        assert!(out.paths.contains("Cargo.toml"), "{}", out.paths);
        assert!(out.paths.contains("src/a.rs"), "{}", out.paths);
    }

    #[tokio::test]
    async fn negation_excludes() {
        let dir = fixture();
        let out = FindTool::new()
            .run(args(&dir, "!src/deep/**"))
            .await
            .unwrap();
        assert!(!out.paths.contains("deep/b.rs"), "{}", out.paths);
        assert!(out.paths.contains("src/a.rs"), "{}", out.paths);
    }

    #[tokio::test]
    async fn pages_partition_the_listing_deterministically() {
        let dir = fixture();
        let mut first = args(&dir, "*.rs");
        first.limit = Some(1);
        let p1 = FindTool::new().run(first).await.unwrap();
        assert_eq!(p1.returned, 1);
        assert_eq!(p1.next_offset, Some(1));

        let mut second = args(&dir, "*.rs");
        second.limit = Some(1);
        second.offset = p1.next_offset;
        let p2 = FindTool::new().run(second).await.unwrap();
        assert_eq!(p2.returned, 1);
        assert_eq!(p2.next_offset, None);
        assert_ne!(p1.paths, p2.paths);
    }

    #[tokio::test]
    async fn an_empty_result_names_the_lever() {
        let dir = fixture();
        let out = FindTool::new().run(args(&dir, "*.zzz")).await.unwrap();
        assert_eq!(out.total, 0);
        assert!(
            out.message.starts_with("No files matched"),
            "{}",
            out.message
        );
        assert!(out.message.contains("no_ignore=true"), "{}", out.message);
    }

    /// Same rule as `grep`'s twin: the two numbers the description quotes are
    /// the two constants above it, and the copy the model reads is the one that
    /// changes what it asks for.
    #[test]
    fn the_description_quotes_the_limits_it_actually_enforces() {
        let description = <FindTool as AlephTool>::DESCRIPTION;
        for (what, value) in [("default limit", DEFAULT_LIMIT), ("max limit", MAX_LIMIT)] {
            assert!(
                description.contains(&value.to_string()),
                "DESCRIPTION never states the {what} ({value})"
            );
        }
    }

    #[tokio::test]
    async fn an_invalid_glob_is_rejected_by_name() {
        let dir = fixture();
        let err = FindTool::new()
            .run(args(&dir, "[unterminated"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Invalid glob"), "{err}");
    }

    #[tokio::test]
    async fn a_protected_location_is_reported_not_silently_dropped() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("creds")).unwrap();
        fs::write(dir.path().join("creds/id_rsa.txt"), "").unwrap();
        fs::write(dir.path().join("ok.txt"), "").unwrap();

        let canonical = dir.path().canonicalize().unwrap();
        let tool = FindTool {
            denied_paths: vec![canonical.join("creds").to_string_lossy().to_string()],
            tool_context_handle: None,
        };
        let out = tool.run(args(&dir, "*.txt")).await.unwrap();
        assert_eq!(out.total, 1);
        assert!(
            out.message.contains("protected-location"),
            "{}",
            out.message
        );
    }
}
