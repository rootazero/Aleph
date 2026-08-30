//! Gitignore-aware filesystem walk shared by `grep` and `find`.
//!
//! # Why this exists
//!
//! Before this module the model had exactly one way to search file *contents*:
//! shell out to `bash`. A `grep -r` there is a context bomb — it does not read
//! `.gitignore`, so one recursive run pours every match under `node_modules/`,
//! `target/` and `dist/` straight into the context window, and nothing further
//! down the pipeline can tell those bytes from the three lines the model wanted.
//!
//! # One walk, two faces
//!
//! [`grep`](super::grep) asks "which lines match" and [`find`](super::find)
//! asks "which files exist", but *"which files does this repository consider
//! its own"* is one question with one answer, so it is answered once, here.
//! The two tools differ only in what they do with the file list they get back.
//!
//! # The deny gate
//!
//! A tree walk is a third face that can read a file's bytes (`file_read` and
//! `bash` are the other two), so it binds the same floor they do — the
//! credential denylist *and* the operator's `[sandbox] deny_read_globs`, which
//! `get_denied_paths()` already merges into one list. The composition is
//! deliberate and split by depth:
//!
//! - the caller-supplied **root** goes through the full
//!   [`check_and_resolve_path`] gate (env / `~` expansion, relative-base
//!   resolution, `FsScope` worktree rebase, canonicalization, denylist,
//!   `/proc` secrets);
//! - every **descendant** the walk yields is checked with [`path_is_denied`] +
//!   [`is_blocked_proc_path`] — the very predicates that gate runs internally.
//!   Descendants need no expansion (they were built by joining onto an already
//!   canonical root), so this is the same derivation evaluated at a cheaper
//!   depth, not a second one.
//!
//! Symlinks are **not followed** (`follow_links(false)`). That is a security
//! property here, not a preference: a followed link could hand the walk a path
//! whose canonical form was never computed, and the descendant check would then
//! be evaluating a spelling rather than a location.
//!
//! # `glob` narrows the result; only `no_ignore` widens the walk
//!
//! The caller's glob is matched against the walk's output rather than handed
//! to the walker, because `ignore`'s override sets have precedence over the
//! ignore rules — see the comment on the `overrides` binding in [`walk`]. The
//! rule the rest of this module and both tool descriptions state is therefore
//! true without exception: `no_ignore` is the only way to reach a file the
//! repository ignores.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;

use crate::builtin_tools::error::ToolError;
use crate::builtin_tools::file_ops::{
    check_and_resolve_path, is_blocked_proc_path, path_is_denied, SKIPPED_DIRS,
};

/// Hard ceiling on files **visited** by one walk — not on files returned.
///
/// Not a user-facing knob: it bounds the worst case (a `path` pointed at `/`)
/// so a single tool call cannot spend the turn budget walking a disk. Every
/// caller reports whether it was hit, so the omission is never silent.
///
/// Visited rather than kept, because `glob` filters the *result*: a ceiling on
/// what came back would have let `glob: "*.zzz"` traverse a million files
/// while reporting nothing withheld.
pub(super) const MAX_WALK_FILES: usize = 40_000;

/// Outcome of one walk. `files` is sorted by path, which is what makes
/// `offset`-based paging across calls stable — an unordered walk would page
/// over a different sequence each time and silently skip results.
#[derive(Debug)]
pub(super) struct WalkReport {
    pub files: Vec<PathBuf>,
    /// [`MAX_WALK_FILES`] files were visited and the walk stopped there;
    /// `files` covers a prefix of the tree. Narrowing `glob` does not lift
    /// this — only a narrower `path` does.
    pub walk_capped: bool,
    /// Entries dropped by the denylist floor (credential dirs, `deny_read_globs`).
    pub denied: usize,
    /// Directories this module's own floor refused to descend into: `.git`,
    /// and the generated/VCS names in [`SKIPPED_DIRS`].
    ///
    /// Deliberately NOT "directories excluded from the walk". `ignore` applies
    /// gitignore rules *before* `filter_entry` runs, so a `target/` listed in
    /// `.gitignore` never reaches this counter — it is already gone. A field
    /// named for the larger set would be a measuring instrument reporting a
    /// number about a different population than its name claims, which is how
    /// a message built from it comes to say something false.
    pub floor_skipped_dirs: usize,
}

/// What to walk and how.
pub(super) struct WalkRequest<'a> {
    /// Caller-supplied, *not yet* resolved. [`walk`] gates it.
    pub path: &'a str,
    /// `rg --glob` semantics: `*.rs`, `src/**/*.rs`, `*.{rs,toml}`, `!*_test.rs`.
    pub glob: Option<&'a str>,
    /// `false` disables `.gitignore` **and** the [`SKIPPED_DIRS`] floor — the
    /// single lever for "yes, I really do want to search `target/`".
    pub respect_ignore: bool,
    /// Merged credential + `deny_read_globs` floor from `get_denied_paths()`.
    pub denied_paths: &'a [String],
    /// Workspace output dir used as the relative-path base, from `ToolContext`.
    pub output_dir: Option<&'a Path>,
}

/// Resolve `req.path`, then walk it.
///
/// Returns the canonical root alongside the report so callers can render paths
/// relative to it without resolving twice.
pub(super) fn walk(req: &WalkRequest<'_>) -> Result<(PathBuf, WalkReport), ToolError> {
    let root = check_and_resolve_path(Path::new(req.path), req.denied_paths, req.output_dir)?;
    if !root.exists() {
        return Err(ToolError::Execution(format!(
            "Path not found: {}",
            req.path
        )));
    }

    // A single file as the root is a legitimate target (grep one file). Short
    // circuiting keeps the glob/ignore machinery from silently filtering away
    // the one thing the caller named by hand.
    if root.is_file() {
        return Ok((
            root.clone(),
            WalkReport {
                files: vec![root],
                walk_capped: false,
                denied: 0,
                floor_skipped_dirs: 0,
            },
        ));
    }

    let mut builder = WalkBuilder::new(&root);
    builder
        .follow_links(false)
        // Dotfiles are content in a source tree (`.github/workflows`,
        // `.cargo/config.toml`), so they are walked — but that is precisely why
        // `.git` needs the explicit filter below: by default it is excluded
        // only *because* it is hidden.
        .hidden(false)
        .parents(true)
        .git_ignore(req.respect_ignore)
        .git_global(req.respect_ignore)
        .git_exclude(req.respect_ignore)
        .ignore(req.respect_ignore)
        .require_git(false)
        .sort_by_file_path(std::cmp::Ord::cmp);

    // Built here, and deliberately NOT handed to `builder.overrides(..)`.
    //
    // In the `ignore` crate an override set takes precedence *over* the ignore
    // rules: a positive pattern whitelists, and a whitelisted file is yielded
    // even when `.gitignore` excludes it. Measured, not assumed — in a tree
    // whose `.gitignore` names `ignored.txt`, `rg -g '*.txt'` lists that file
    // and a bare `rg` does not.
    //
    // That is a reasonable default for a person typing `-g '*.log'` at a
    // prompt. It is the wrong one here, because it makes `glob` a second,
    // undocumented spelling of `no_ignore` — and a silent one: the result
    // would still carry `notes::ignored`'s "ignored and generated files were
    // excluded; pass no_ignore=true to search them", which in that case is
    // false. `no_ignore` stays the single lever for widening; `glob` narrows
    // what the walk yields and never widens what it reaches.
    //
    // Moving the match out of the walker costs no pruning, which is the only
    // thing the walker could have done with it: a positive override never
    // prunes a directory (`Override::matched` declines to ignore a directory
    // that merely fails to match), so `glob: "*.rs"` already visited every
    // entry in the tree before this.
    let overrides =
        match req.glob {
            Some(pattern) => {
                let mut glob_builder = OverrideBuilder::new(&root);
                glob_builder.add(pattern).map_err(|e| {
                    ToolError::InvalidArgs(format!("Invalid glob '{pattern}': {e}"))
                })?;
                Some(glob_builder.build().map_err(|e| {
                    ToolError::InvalidArgs(format!("Invalid glob '{pattern}': {e}"))
                })?)
            }
            None => None,
        };

    let floor_skipped = Arc::new(AtomicUsize::new(0));
    let respect_ignore = req.respect_ignore;
    let root_for_filter = root.clone();
    let skipped_for_filter = Arc::clone(&floor_skipped);
    builder.filter_entry(move |entry| {
        if !entry.file_type().is_some_and(|t| t.is_dir()) {
            return true;
        }
        // Never the repository's own object store. `.git` survives
        // `hidden(false)` and holds more files than the tree it describes.
        let name = entry.file_name().to_string_lossy();
        if name == ".git" {
            skipped_for_filter.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        if !respect_ignore {
            return true;
        }
        // Generated/VCS floor for trees with no `.gitignore` to speak for them
        // (a downloads folder, a vendored drop). Descendants only — naming
        // `target/` as the root is an explicit intent, and a filter that
        // second-guessed it would answer a question nobody asked.
        if entry.path() != root_for_filter && SKIPPED_DIRS.contains(&name.as_ref()) {
            skipped_for_filter.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        true
    });

    let mut files = Vec::new();
    let mut denied = 0usize;
    let mut walk_capped = false;
    let mut visited = 0usize;

    for entry in builder.build() {
        let entry = match entry {
            Ok(e) => e,
            // A directory the process cannot read is a fact about permissions,
            // not about the tree's contents: skip the entry, keep the walk.
            Err(e) => {
                tracing::debug!(error = %e, "file_search: walk entry error");
                continue;
            }
        };
        // Symlinks (unfollowed), sockets and fifos have nothing to read.
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }

        let path = entry.into_path();

        // The cap counts files VISITED, not files kept. It bounds the work one
        // call may do, and since `glob` (above) narrows the result without
        // narrowing the walk, a pattern that matches almost nothing must not
        // license an unbounded traversal. Counting kept files instead was the
        // same shape as a page size: a bound on the answer, not on the search.
        if visited >= MAX_WALK_FILES {
            walk_capped = true;
            break;
        }
        visited += 1;

        if path_is_denied(&path, req.denied_paths) || is_blocked_proc_path(&path) {
            denied = denied.saturating_add(1);
            continue;
        }
        if overrides
            .as_ref()
            .is_some_and(|set| set.matched(&path, false).is_ignore())
        {
            continue;
        }
        files.push(path);
    }

    Ok((
        root,
        WalkReport {
            files,
            walk_capped,
            denied,
            floor_skipped_dirs: floor_skipped.load(Ordering::Relaxed),
        },
    ))
}

/// Render `path` for display: relative to `root` when it sits underneath,
/// absolute otherwise, always with `/` separators so the output a model reads
/// is byte-identical across platforms.
pub(super) fn display_path(path: &Path, root: &Path) -> String {
    let rendered = path
        .strip_prefix(root)
        .map_or_else(|_| path.to_path_buf(), Path::to_path_buf);
    rendered.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn req<'a>(path: &'a str, glob: Option<&'a str>, denied: &'a [String]) -> WalkRequest<'a> {
        WalkRequest {
            path,
            glob,
            respect_ignore: true,
            denied_paths: denied,
            output_dir: None,
        }
    }

    fn names(report: &WalkReport, root: &Path) -> Vec<String> {
        report.files.iter().map(|p| display_path(p, root)).collect()
    }

    #[test]
    fn gitignored_files_are_not_walked() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(".gitignore"), "ignored.txt\nbuild/\n").unwrap();
        fs::write(dir.path().join("kept.txt"), "a").unwrap();
        fs::write(dir.path().join("ignored.txt"), "b").unwrap();
        fs::create_dir(dir.path().join("build")).unwrap();
        fs::write(dir.path().join("build/out.txt"), "c").unwrap();

        let root = dir.path().to_string_lossy().to_string();
        let (canonical, report) = walk(&req(&root, None, &[])).unwrap();
        let names = names(&report, &canonical);

        assert!(names.contains(&"kept.txt".to_string()), "{names:?}");
        assert!(!names.contains(&"ignored.txt".to_string()), "{names:?}");
        assert!(!names.iter().any(|n| n.starts_with("build/")), "{names:?}");
    }

    #[test]
    fn no_ignore_lifts_gitignore_and_the_generated_floor() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(dir.path().join("ignored.txt"), "b").unwrap();
        fs::create_dir(dir.path().join("node_modules")).unwrap();
        fs::write(dir.path().join("node_modules/dep.js"), "c").unwrap();

        let root = dir.path().to_string_lossy().to_string();
        let mut request = req(&root, None, &[]);
        request.respect_ignore = false;
        let (canonical, report) = walk(&request).unwrap();
        let names = names(&report, &canonical);

        assert!(names.contains(&"ignored.txt".to_string()), "{names:?}");
        assert!(
            names.contains(&"node_modules/dep.js".to_string()),
            "{names:?}"
        );
    }

    #[test]
    fn generated_dirs_are_skipped_without_a_gitignore() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join("node_modules")).unwrap();
        fs::write(dir.path().join("node_modules/dep.js"), "c").unwrap();
        fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();
        // The walker honours .gitignore files in PARENTS of the root
        // (`parents(true)`), and some environments ship a $TMPDIR/.gitignore
        // that already names `node_modules/` — which skips the dir before the
        // floor filter ever sees it, leaving `floor_skipped_dirs` at 0 for a
        // reason the test did not arrange. A local negation keeps the dir
        // visible to the gitignore layer so the skip measured below is the
        // floor filter's own work, not the ambient environment's.
        fs::write(dir.path().join(".gitignore"), "!node_modules/\n").unwrap();

        let root = dir.path().to_string_lossy().to_string();
        let (canonical, report) = walk(&req(&root, None, &[])).unwrap();

        assert_eq!(
            names(&report, &canonical),
            vec![".gitignore".to_string(), "main.rs".to_string()]
        );
        assert_eq!(report.floor_skipped_dirs, 1);
    }

    #[test]
    fn glob_filters_by_extension_at_any_depth() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src/deep")).unwrap();
        fs::write(dir.path().join("src/a.rs"), "x").unwrap();
        fs::write(dir.path().join("src/deep/b.rs"), "x").unwrap();
        fs::write(dir.path().join("src/c.toml"), "x").unwrap();

        let root = dir.path().to_string_lossy().to_string();
        let (canonical, report) = walk(&req(&root, Some("*.rs"), &[])).unwrap();
        let mut found = names(&report, &canonical);
        found.sort();

        assert_eq!(
            found,
            vec!["src/a.rs".to_string(), "src/deep/b.rs".to_string()]
        );
    }

    /// The lever rule from the other side: a `glob` must not reach a file the
    /// repository ignores.
    ///
    /// `ignore`'s override sets whitelist by default — a positive pattern
    /// beats `.gitignore`, which is why `rg -g '*.txt'` lists a file a bare
    /// `rg` hides. Handing the glob to the walker therefore made `glob` a
    /// silent second spelling of `no_ignore`, while the message still told the
    /// caller that ignored files had been excluded. Move the match back into
    /// `WalkBuilder::overrides` and this is the test that says so.
    #[test]
    fn a_glob_does_not_resurrect_an_ignored_file() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(dir.path().join("ignored.txt"), "x").unwrap();
        fs::write(dir.path().join("kept.txt"), "x").unwrap();

        let root = dir.path().to_string_lossy().to_string();
        let (canonical, report) = walk(&req(&root, Some("*.txt"), &[])).unwrap();
        assert_eq!(names(&report, &canonical), vec!["kept.txt".to_string()]);
    }

    /// The directory half of the same rule. `**/*` matches directory paths as
    /// well as file paths, so as an override it whitelisted — and re-opened —
    /// every tree `.gitignore` had pruned.
    ///
    /// The ignored directory is deliberately NOT one of [`SKIPPED_DIRS`]. Named
    /// `build/`, this test passed against the un-fixed code: the generated-dir
    /// floor caught the directory before the override could resurrect it, so
    /// the assertion held for a reason that has nothing to do with what it
    /// claims to be about. Only a name that `.gitignore` alone excludes puts
    /// the override precedence on trial.
    #[test]
    fn a_wildcard_glob_does_not_resurrect_an_ignored_tree() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(".gitignore"), "artifacts/\n").unwrap();
        fs::create_dir(dir.path().join("artifacts")).unwrap();
        fs::write(dir.path().join("artifacts/out.txt"), "x").unwrap();
        fs::write(dir.path().join("kept.txt"), "x").unwrap();

        let root = dir.path().to_string_lossy().to_string();
        let (canonical, report) = walk(&req(&root, Some("**/*"), &[])).unwrap();
        let names = names(&report, &canonical);
        assert!(
            !names.iter().any(|n| n.starts_with("artifacts/")),
            "{names:?}"
        );
        assert!(names.contains(&"kept.txt".to_string()), "{names:?}");
    }

    /// Negation survived the move out of the walker. Both tool descriptions
    /// advertise `!*_test.rs`, and an override set matched by hand answers
    /// `Ignore` for a negated hit and `None` — keep it — for everything else,
    /// which is only true while the set has no positive patterns in it.
    #[test]
    fn a_negated_glob_still_excludes_and_keeps_the_rest() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "x").unwrap();
        fs::write(dir.path().join("a_test.rs"), "x").unwrap();
        fs::write(dir.path().join("notes.md"), "x").unwrap();

        let root = dir.path().to_string_lossy().to_string();
        let (canonical, report) = walk(&req(&root, Some("!*_test.rs"), &[])).unwrap();
        let mut found = names(&report, &canonical);
        found.sort();
        assert_eq!(found, vec!["a.rs".to_string(), "notes.md".to_string()]);
    }

    #[test]
    fn brace_alternates_are_one_glob() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "x").unwrap();
        fs::write(dir.path().join("b.toml"), "x").unwrap();
        fs::write(dir.path().join("c.md"), "x").unwrap();

        let root = dir.path().to_string_lossy().to_string();
        let (canonical, report) = walk(&req(&root, Some("*.{rs,toml}"), &[])).unwrap();
        let mut found = names(&report, &canonical);
        found.sort();

        assert_eq!(found, vec!["a.rs".to_string(), "b.toml".to_string()]);
    }

    /// The floor this module exists to honour: a walk is a third face that
    /// reads bytes, and it binds the same denylist `file_read` does.
    #[test]
    fn denied_descendants_are_dropped_and_counted() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join("secrets")).unwrap();
        fs::write(dir.path().join("secrets/key.pem"), "PRIVATE").unwrap();
        fs::write(dir.path().join("ok.txt"), "fine").unwrap();

        let canonical_root = dir.path().canonicalize().unwrap();
        let denied = vec![canonical_root.join("secrets").to_string_lossy().to_string()];
        let root = dir.path().to_string_lossy().to_string();
        let (canonical, report) = walk(&req(&root, None, &denied)).unwrap();

        assert_eq!(names(&report, &canonical), vec!["ok.txt".to_string()]);
        assert_eq!(report.denied, 1);
    }

    #[test]
    fn a_file_root_yields_exactly_that_file() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("one.txt");
        fs::write(&file, "x").unwrap();

        let path = file.to_string_lossy().to_string();
        let (_, report) = walk(&req(&path, None, &[])).unwrap();
        assert_eq!(report.files.len(), 1);
    }

    #[test]
    fn walk_order_is_stable_across_calls() {
        let dir = tempdir().unwrap();
        for name in ["c.txt", "a.txt", "b.txt"] {
            fs::write(dir.path().join(name), "x").unwrap();
        }
        let root = dir.path().to_string_lossy().to_string();
        let first = walk(&req(&root, None, &[])).unwrap().1.files;
        let second = walk(&req(&root, None, &[])).unwrap().1.files;
        assert_eq!(first, second);
        assert!(first.windows(2).all(|w| w[0] <= w[1]), "{first:?}");
    }

    #[test]
    fn missing_path_is_a_named_error_not_an_empty_result() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("nope").to_string_lossy().to_string();
        let err = walk(&req(&missing, None, &[])).unwrap_err();
        assert!(err.to_string().contains("not found"), "{err}");
    }

    /// `.git` stays out even with every ignore mechanism turned off.
    ///
    /// This is the arm that makes the explicit `.git` filter load-bearing. The
    /// default path does not: mutating the filter away left the sibling test
    /// below green, because `ignore`'s own machinery already drops `.git` while
    /// the git features are on. Turn them off — which is exactly what
    /// `no_ignore: true` does — and the object store is the largest directory
    /// in the tree.
    #[test]
    fn dot_git_stays_out_even_with_no_ignore() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".git/objects")).unwrap();
        fs::write(dir.path().join(".git/objects/blob"), "x").unwrap();
        fs::write(dir.path().join("main.rs"), "x").unwrap();

        let root = dir.path().to_string_lossy().to_string();
        let mut request = req(&root, None, &[]);
        request.respect_ignore = false;
        let (canonical, report) = walk(&request).unwrap();
        let found = names(&report, &canonical);

        assert_eq!(found, vec!["main.rs".to_string()], "{found:?}");
    }

    #[test]
    fn dot_git_is_never_walked_even_though_dotfiles_are() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join(".git/HEAD"), "ref: x").unwrap();
        fs::write(dir.path().join(".env.example"), "K=V").unwrap();

        let root = dir.path().to_string_lossy().to_string();
        let (canonical, report) = walk(&req(&root, None, &[])).unwrap();
        let found = names(&report, &canonical);

        assert!(found.contains(&".env.example".to_string()), "{found:?}");
        assert!(!found.iter().any(|n| n.starts_with(".git/")), "{found:?}");
    }
}
