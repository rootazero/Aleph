//! Self-deleting scratch directories for tests.
//!
//! Every test that needs "a fresh path on disk" used to write
//! `std::env::temp_dir().join(unique_name)` by hand. That form has no owner:
//! nothing removes the tree afterwards, so each run adds to a pile that only
//! grows. One developer machine had accumulated 4 987 entries (3.8 GB) under
//! `$TMPDIR` this way, across 60 files.
//!
//! [`scratch_root`] is the single replacement. It is deliberately shaped around
//! the two ways the obvious fix goes wrong — both of which stay green.

use std::path::PathBuf;

use tempfile::TempDir;

/// A scratch directory that deletes itself, plus a path *inside* it.
///
/// Returns `(guard, path)`. Bind **both** in the frame that uses them:
///
/// ```ignore
/// let (_scratch, dir) = scratch_root();
/// std::fs::create_dir_all(&dir).unwrap();
/// ```
///
/// # Why the path is a child, not the guard's own directory
///
/// `tempfile::tempdir()` *creates* the directory it hands back, while a large
/// share of these call sites depend on the path **not existing yet** — that is
/// exactly what makes `SqliteMemoryBackend::new` treat its argument as the
/// database *file* rather than as a directory to open inside. Handing back a
/// child keeps both classes of caller byte-identical to the hand-rolled form
/// they replace, while still giving the tree an owner.
///
/// # Why the guard must be bound by the caller
///
/// Dropping the guard removes the tree. A helper that binds the guard locally
/// and returns only the path deletes everything *before its caller runs* — and
/// the tests still pass, because SQLite keeps writing through the file
/// descriptor it already holds on the now-unlinked file. Name it `_scratch`,
/// never `_`: a bare `_` pattern drops immediately.
#[must_use]
pub fn scratch_root() -> (TempDir, PathBuf) {
    let guard = tempfile::tempdir().expect("create scratch tempdir");
    let path = guard.path().join("root");
    (guard, path)
}

// =============================================================================
// Process-exit reaping
// =============================================================================

/// Delete `dir` when this process exits, and kill `pid` first if one is given.
///
/// # The problem this exists for
///
/// Some test scaffolding genuinely must outlive every frame: a server parked in
/// a `static OnceCell` so one instance serves the whole binary, a `OnceLock`
/// root shared by sibling tests, a `LazyLock` store. **A static never drops**,
/// so `impl Drop` never runs — which is why these sites reached for
/// `mem::forget` or `TempDir::keep()` and left one abandoned tree (and, for the
/// probe harnesses, one live `aleph-server` bound to a random port) behind
/// every single run.
///
/// `Drop` is the wrong tool for something a static owns. `atexit` is the right
/// one: it fires both on a normal `main` return and on `std::process::exit`,
/// which is how libtest ends a failing run.
///
/// Unix only — `libc::kill` has no portable twin. On other platforms
/// [`keep_until_exit`] falls back to the previous leak-forever behaviour rather
/// than pretending, and the process reaper has no non-unix callers.
#[cfg(unix)]
fn register_for_exit(pid: Option<u32>, dir: PathBuf) {
    use std::sync::{Mutex, OnceLock};

    static DOOMED: Mutex<Vec<(Option<u32>, PathBuf)>> = Mutex::new(Vec::new());
    static REGISTERED: OnceLock<()> = OnceLock::new();

    extern "C" fn reap() {
        let Ok(mut doomed) = DOOMED.lock() else {
            return;
        };
        for (pid, dir) in doomed.drain(..) {
            if let Some(pid) = pid {
                // SAFETY: `kill` on a pid this process spawned. A stale pid
                // gets ESRCH, which is ignored — this is best-effort cleanup.
                unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    DOOMED
        .lock()
        .expect("scratch reaper registry")
        .push((pid, dir));
    REGISTERED.get_or_init(|| {
        // SAFETY: registering a handler that takes no arguments and captures
        // nothing; `atexit` is documented to accept at least 32 of them.
        unsafe { libc::atexit(reap) };
    });
}

/// Kill `pid` and delete `dir` when this process exits. See
/// [`register_for_exit`] for why `Drop` cannot do this job.
#[cfg(unix)]
pub fn reap_on_exit(pid: u32, dir: PathBuf) {
    register_for_exit(Some(pid), dir);
}

/// Hand a scratch directory to something that outlives every frame — a
/// `static`, a `LazyLock`, a registry shared by a whole test binary — without
/// abandoning it.
///
/// Disarms the guard (the caller keeps the path) but registers the tree for
/// removal at process exit. This is the honest replacement for `mem::forget` /
/// `TempDir::keep()`, whose justification was always "acceptable in a test
/// binary" — true per run, and false by the four-thousandth one.
#[must_use]
pub fn keep_until_exit(dir: TempDir) -> PathBuf {
    #[cfg(unix)]
    {
        let path = dir.keep();
        register_for_exit(None, path.clone());
        path
    }
    #[cfg(not(unix))]
    {
        dir.keep()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_returned_path_does_not_exist_yet() {
        let (_scratch, path) = scratch_root();
        assert!(
            !path.exists(),
            "callers that pass this to something which branches on existence \
             (SqliteMemoryBackend::new) depend on it being absent"
        );
        assert!(path.parent().expect("has a parent").exists());
    }

    #[test]
    fn dropping_the_guard_removes_the_tree() {
        let (guard, path) = scratch_root();
        std::fs::create_dir_all(&path).expect("create");
        std::fs::write(path.join("f"), b"x").expect("write");
        let parent = guard.path().to_path_buf();
        drop(guard);
        assert!(
            !parent.exists(),
            "the whole point: the tree has an owner and the owner cleans up"
        );
    }

    /// Every `scratch_root()` caller must dispose of the guard in one of
    /// **three** ways: BE a test, hand the guard back to its caller, or hand it
    /// to [`keep_until_exit`]. A helper that binds the guard locally and
    /// returns only the path deletes the tree before its caller runs — and
    /// nothing fails, because SQLite keeps writing through the descriptor it
    /// already holds and the code under test simply re-creates the directories
    /// it needs.
    ///
    /// That is not a hypothetical: it was found and fixed once in
    /// `dreaming::note_weave`, and four more helpers in the same subsystem
    /// were still doing it a round later. A fix that only covers the instances
    /// you happened to read is not a fix for the class — hence this.
    ///
    /// # The third disposition (added 2026-09-07)
    ///
    /// The rule shipped enumerating two, because on the day it was written
    /// those were the only two that existed — and an enumeration written that
    /// way reads afterwards as if it were exhaustive (判据 §5). The third is
    /// legitimate for the same reason the other two are: **the tree outlives
    /// the caller.** `keep_until_exit` consumes the `TempDir`, disarms its
    /// `Drop`, and registers the path for removal at process exit, which is
    /// exactly what a helper feeding a `OnceLock`/`static` needs — a static
    /// never drops, so returning the guard would be useless and dropping it
    /// would delete the tree the static is about to use.
    ///
    /// It was found by `install_test_tool_result_store`
    /// (`tools/result_store.rs`) going red while being correct. Routing that
    /// helper around the scanner was the wrong fix: a guard that misfires is
    /// more expensive than one that stays quiet, because it gets cited as
    /// evidence, and it would have pushed every future caller away from
    /// `scratch_root()` to keep a scanner happy.
    ///
    /// Source-level, because at runtime "the guard was dropped early" and "the
    /// guard did its job" look identical.
    #[test]
    fn no_helper_drops_the_scratch_guard_before_returning() {
        let offenders = scan_for_helpers_that_swallow_the_guard();
        assert!(
            offenders.is_empty(),
            "these fns call scratch_root() but neither are tests nor return the \
             TempDir, so the tree is deleted when they return:\n  {}",
            offenders.join("\n  ")
        );
    }

    /// Walks `src/` looking for `fn` items whose body mentions `scratch_root()`
    /// and hands each file to [`offenders_in`]. One report per file, as before.
    fn scan_for_helpers_that_swallow_the_guard() -> Vec<String> {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        walk(&root, &mut files);
        files.sort();

        let mut offenders = Vec::new();
        for file in files {
            let Ok(raw) = std::fs::read_to_string(&file) else {
                continue;
            };
            // One report per file: the first offender names the problem, and a
            // file with two of them is one edit anyway.
            if let Some((line_no, sig)) = offenders_in(&raw).into_iter().next() {
                offenders.push(format!(
                    "{}:{} — {}",
                    file.strip_prefix(env!("CARGO_MANIFEST_DIR"))
                        .unwrap_or(&file)
                        .display(),
                    line_no,
                    sig
                ));
            }
        }
        offenders
    }

    /// Every `fn` in `source` that calls `scratch_root()` and disposes of the
    /// guard in none of the three legitimate ways, as `(1-based header line,
    /// signature)`.
    ///
    /// Comment lines are stripped first: a scanner that judges prose is judging
    /// documentation, not code.
    ///
    /// # What this can and cannot express
    ///
    /// It is line-oriented, so the `keep_until_exit` disposition is recognised
    /// only as a **direct hand-off of the binding `scratch_root()` returned** —
    /// `let (g, _) = scratch_root(); … keep_until_exit(g)`. Deliberately NOT
    /// the bare substring: `keep_until_exit(some_other_tempdir)` sitting beside
    /// a dropped guard is exactly the shape a substring check would bless, and
    /// this rule exists because that shape shipped once already.
    ///
    /// The narrowness that buys is real and worth stating: laundering the guard
    /// through another local (`let g2 = g; keep_until_exit(g2)`) reads as an
    /// offender here. That is the fail-closed direction — a false red is
    /// answered by a person, a false green is not — and no caller in the tree
    /// does it.
    fn offenders_in(source: &str) -> Vec<(usize, String)> {
        // `\r` first: this repo is checked out CRLF on Windows, and a
        // scanner anchored to bare `\n` matches nothing there.
        let text = source.replace('\r', "");
        let lines: Vec<&str> = text.lines().collect();
        let code: Vec<&str> = lines
            .iter()
            .map(|l| {
                if l.trim_start().starts_with("//") {
                    ""
                } else {
                    *l
                }
            })
            .collect();

        let mut found = Vec::new();
        for (i, line) in code.iter().enumerate() {
            if !line.contains("scratch_root()") || line.contains("fn scratch_root") {
                continue;
            }
            // Nearest enclosing `fn`: walk back to the last `fn` header
            // whose brace-depth region still contains line `i`.
            let Some(start) = enclosing_fn(&code, i) else {
                continue;
            };
            // The signature can span lines; read until the opening brace.
            let mut sig = String::new();
            for l in code.iter().skip(start) {
                sig.push_str(l);
                sig.push(' ');
                if l.contains('{') {
                    break;
                }
            }
            let is_test = (1..=6).any(|back| {
                start
                    .checked_sub(back)
                    .is_some_and(|k| lines[k].contains("#[test]") || lines[k].contains("::test]"))
            });
            // Third disposition: the guard THIS line bound is handed to
            // `keep_until_exit` somewhere later in the same `fn`.
            let kept = guard_binding(line).is_some_and(|guard| {
                let end = fn_body_end(&code, start);
                code[i..=end]
                    .iter()
                    .any(|l| hands_guard_to_keep_until_exit(l, guard))
            });
            if is_test || sig.contains("TempDir") || kept {
                continue;
            }
            found.push((start + 1, sig.trim().to_string()));
        }
        found
    }

    /// The identifier bound to the guard on a
    /// `let (guard, path) = …scratch_root();` line, or `None` when the line
    /// binds none — a bare `_` (which drops immediately, and is what
    /// [`scratch_root`]'s own doc warns about) or any shape this scanner does
    /// not read. `None` means "no third disposition available", never "fine".
    fn guard_binding(line: &str) -> Option<&str> {
        let inside = line.split_once("let ")?.1.split_once('(')?.1;
        let name = inside
            .split(',')
            .next()?
            .trim()
            .trim_start_matches("mut ")
            .trim();
        (!name.is_empty() && name != "_" && name.chars().all(|c| c.is_alphanumeric() || c == '_'))
            .then_some(name)
    }

    /// Whether `line` passes exactly `guard` to `keep_until_exit` — the
    /// argument, not the mere presence of the call.
    fn hands_guard_to_keep_until_exit(line: &str, guard: &str) -> bool {
        line.split("keep_until_exit(").skip(1).any(|rest| {
            rest.trim_start()
                .strip_prefix(guard)
                .is_some_and(|tail| tail.trim_start().starts_with(')'))
        })
    }

    /// Last line of the `fn` item whose header is at `start`, by brace depth.
    /// Falls back to the end of the file for an unbalanced tail rather than
    /// silently reporting an empty range (which would read as "not kept").
    fn fn_body_end(code: &[&str], start: usize) -> usize {
        let mut depth: i32 = 0;
        let mut opened = false;
        for (i, line) in code.iter().enumerate().skip(start) {
            depth += line.matches('{').count() as i32;
            if depth > 0 {
                opened = true;
            }
            depth -= line.matches('}').count() as i32;
            if opened && depth <= 0 {
                return i;
            }
        }
        code.len().saturating_sub(1)
    }

    /// Index of the `fn` header enclosing `target`, by brace depth.
    fn enclosing_fn(code: &[&str], target: usize) -> Option<usize> {
        let mut candidate = None;
        let mut depth: i32 = 0;
        let mut pending: Vec<(usize, i32)> = Vec::new();
        for (i, line) in code.iter().enumerate().take(target + 1) {
            if line.contains(" fn ") || line.trim_start().starts_with("fn ") {
                pending.push((i, depth));
            }
            depth += line.matches('{').count() as i32;
            depth -= line.matches('}').count() as i32;
            while let Some(&(_, d)) = pending.last() {
                if depth <= d && i > pending.last().expect("checked").0 {
                    pending.pop();
                } else {
                    break;
                }
            }
            if let Some(&(idx, _)) = pending.last() {
                candidate = Some(idx);
            }
        }
        candidate
    }

    /// The scanner, judged against source it does not have to go looking for.
    ///
    /// Without this the only evidence any disposition works is whichever real
    /// caller happens to have that shape today — and the third disposition
    /// exists because it had exactly one, which is how a rule ends up being
    /// tested by its own motivating example (判据 §3: a guard's green covers
    /// only the shapes it recognises, so make it meet them all on purpose).
    ///
    /// Each case is a whole synthetic file, because `offenders_in` reads
    /// enclosing-`fn` structure, not single lines.
    #[test]
    fn the_scanner_accepts_all_three_dispositions_and_still_catches_the_swallow() {
        let cases: &[(&str, bool, &str)] = &[
            (
                "returns the guard",
                false,
                "fn helper() -> (TempDir, PathBuf) {\n    let (guard, path) = scratch_root();\n    (guard, path)\n}\n",
            ),
            (
                "is a test",
                false,
                "#[test]\nfn a_test() {\n    let (_scratch, path) = scratch_root();\n    assert!(!path.exists());\n}\n",
            ),
            (
                "hands the guard to keep_until_exit",
                false,
                "fn install() -> PathBuf {\n    let (scratch, base) = scratch_root();\n    let _kept = crate::utils::scratch::keep_until_exit(scratch);\n    base\n}\n",
            ),
            (
                "swallows the guard",
                true,
                "fn helper() -> PathBuf {\n    let (_scratch, path) = scratch_root();\n    path\n}\n",
            ),
            (
                // The reason the check reads the ARGUMENT: a call that keeps
                // some OTHER tempdir alive says nothing about this guard, and
                // a substring check would bless it.
                "keeps a different tempdir while swallowing the guard",
                true,
                "fn helper() -> PathBuf {\n    let (_scratch, path) = scratch_root();\n    let other = tempfile::tempdir().unwrap();\n    let _ = keep_until_exit(other);\n    path\n}\n",
            ),
            (
                // A bare `_` binds nothing, so there is no guard to hand on —
                // the call below cannot be talking about it.
                "drops the guard into a bare underscore",
                true,
                "fn helper() -> PathBuf {\n    let (_, path) = scratch_root();\n    let _ = keep_until_exit(path.clone());\n    path\n}\n",
            ),
            (
                // Prefix, not identity: `scratch2` is a different binding.
                "hands on a binding whose name merely starts the same",
                true,
                "fn helper() -> PathBuf {\n    let (scratch, base) = scratch_root();\n    let _ = keep_until_exit(scratch2);\n    base\n}\n",
            ),
            (
                // The disposition must be inside the SAME fn — a later
                // neighbour's `keep_until_exit` is not this fn's business.
                "leaves the hand-off in the next function",
                true,
                "fn helper() -> PathBuf {\n    let (scratch, base) = scratch_root();\n    base\n}\n\nfn other(scratch: TempDir) {\n    let _ = keep_until_exit(scratch);\n}\n",
            ),
        ];

        for (name, should_flag, src) in cases {
            let flagged = !offenders_in(src).is_empty();
            assert_eq!(
                flagged, *should_flag,
                "case {name:?}: expected flagged={should_flag}, got {flagged}\n{src}"
            );
        }
    }

    /// The argument check on its own, at the boundaries the fixture above
    /// exercises only end to end.
    #[test]
    fn the_keep_until_exit_check_reads_the_argument() {
        assert!(hands_guard_to_keep_until_exit(
            "    let _kept = crate::utils::scratch::keep_until_exit(scratch);",
            "scratch"
        ));
        assert!(hands_guard_to_keep_until_exit(
            "    keep_until_exit( scratch )",
            "scratch"
        ));
        assert!(!hands_guard_to_keep_until_exit(
            "    let _ = keep_until_exit(scratch_two);",
            "scratch"
        ));
        assert!(!hands_guard_to_keep_until_exit(
            "    let _ = keep_until_exit(other);",
            "scratch"
        ));
        assert!(!hands_guard_to_keep_until_exit("    let x = 1;", "scratch"));

        assert_eq!(
            guard_binding("    let (scratch, base) = scratch_root();"),
            Some("scratch")
        );
        assert_eq!(
            guard_binding("    let (mut g, p) = scratch_root();"),
            Some("g")
        );
        assert_eq!(guard_binding("    let (_, path) = scratch_root();"), None);
        assert_eq!(guard_binding("    let path = scratch_root().1;"), None);
    }
}
