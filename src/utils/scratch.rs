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

    /// Every `scratch_root()` caller must either BE a test or hand the guard
    /// on. A helper that binds the guard locally and returns only the path
    /// deletes the tree before its caller runs — and nothing fails, because
    /// SQLite keeps writing through the descriptor it already holds and the
    /// code under test simply re-creates the directories it needs.
    ///
    /// That is not a hypothetical: it was found and fixed once in
    /// `dreaming::note_weave`, and four more helpers in the same subsystem
    /// were still doing it a round later. A fix that only covers the instances
    /// you happened to read is not a fix for the class — hence this.
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

    /// Walks `src/` looking for `fn` items whose body mentions `scratch_root()`.
    /// Comment lines are stripped first: a scanner that judges prose is judging
    /// documentation, not code.
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
            // `\r` first: this repo is checked out CRLF on Windows, and a
            // scanner anchored to bare `\n` matches nothing there.
            let text = raw.replace('\r', "");
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
                    start.checked_sub(back).is_some_and(|k| {
                        lines[k].contains("#[test]") || lines[k].contains("::test]")
                    })
                });
                if is_test || sig.contains("TempDir") {
                    continue;
                }
                offenders.push(format!(
                    "{}:{} — {}",
                    file.strip_prefix(env!("CARGO_MANIFEST_DIR"))
                        .unwrap_or(&file)
                        .display(),
                    start + 1,
                    sig.trim()
                ));
                break;
            }
        }
        offenders
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
}
