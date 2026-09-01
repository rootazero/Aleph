//! Official-content git sync — clone the external skills/plugins repos into an
//! isolated checkout, or hard-reset an existing checkout to `origin/main`.
//! Uses git2 (libgit2, vendored) so no system `git` is required. Network I/O is
//! blocking — call from `spawn_blocking`. Never panics; returns `Err` so the
//! caller can fall back to the embedded snapshot.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use tracing::info;

/// In-process single-flight table keyed by `checkout_dir`. Two concurrent
/// `clone_or_update_at` calls for the same checkout would otherwise both
/// observe `existed = false` and both call `Repository::clone`, racing
/// the second into libgit2 errors that surface as a confusing "lock
/// held by another process" message. Holding the per-dir mutex across
/// the open-or-clone decision is cheap (no IO on the hot path) and
/// closes the TOCTOU. The `static` is a single global; a `HashMap`
/// inside lets us keep many distinct checkouts unblocked. The inner
/// value is `Arc<Mutex<()>>` so callers can clone the guard out of
/// the table and drop the table lock before holding the per-dir one.
static CHECKOUT_LOCKS: OnceLock<Mutex<std::collections::HashMap<PathBuf, Arc<Mutex<()>>>>> =
    OnceLock::new();

/// Clone `repo_url` (branch `main`) into `checkout_dir` if absent; otherwise
/// fetch and hard-reset the working tree to `origin/main`. The checkout dir is
/// official-content-only and never user-edited, so a hard reset is conflict-free.
pub(crate) fn clone_or_update(repo_url: &str, checkout_dir: &Path) -> Result<(), String> {
    clone_or_update_at(repo_url, checkout_dir, None)
}

/// Materialize `repo_url` in `checkout_dir` at a chosen revision.
///
/// - `git_ref: None` — track the default branch: hard-reset to `origin/main` on
///   every call (the official-content path).
/// - `git_ref: Some(rev)` — check out exactly that revision (tag, `origin/<branch>`,
///   or a full commit SHA) with HEAD detached. A pin is *not* re-pointed by later
///   calls, which is the whole point of pinning: the same `rev` always yields the
///   same tree. Refs unknown to the local checkout trigger one refresh, then retry.
pub(crate) fn clone_or_update_at(
    repo_url: &str,
    checkout_dir: &Path,
    git_ref: Option<&str>,
) -> Result<(), String> {
    // Per-checkout_dir single-flight. Acquires the global table lock
    // briefly to look up (or insert) the per-dir mutex, then drops the
    // table lock and holds only the per-dir one across the open-or-clone
    // decision so distinct checkouts stay unblocked.
    let table = CHECKOUT_LOCKS.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let per_dir_mutex = {
        let mut guard = table.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .entry(checkout_dir.to_path_buf())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    };
    let _flight = per_dir_mutex.lock().unwrap_or_else(|e| e.into_inner());
    let existed = checkout_dir.join(".git").exists();
    let repo = if existed {
        git2::Repository::open(checkout_dir)
            .map_err(|e| format!("open {}: {e}", checkout_dir.display()))?
    } else {
        if let Some(parent) = checkout_dir.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
        }
        info!(url = %repo_url, dest = %checkout_dir.display(), "Cloning official content");
        git2::Repository::clone(repo_url, checkout_dir)
            .map_err(|e| format!("clone {repo_url}: {e}"))?
    };
    match git_ref {
        Some(rev) => {
            checkout_pinned(&repo, rev).map_err(|e| format!("checkout '{rev}' of {repo_url}: {e}"))
        }
        // A fresh clone is already on the default branch; only an existing
        // checkout needs the fetch + reset.
        None if existed => {
            update_existing_repo(&repo).map_err(|e| format!("update {repo_url}: {e}"))
        }
        None => Ok(()),
    }
}

/// Check out an immutable revision with HEAD detached, refreshing remote refs
/// once if the revision is not yet known locally.
fn checkout_pinned(repo: &git2::Repository, rev: &str) -> Result<(), git2::Error> {
    let object = match repo.revparse_single(rev) {
        Ok(o) => o,
        Err(_) => {
            let mut remote = repo.find_remote("origin")?;
            remote.fetch(
                &[
                    "+refs/heads/*:refs/remotes/origin/*",
                    "+refs/tags/*:refs/tags/*",
                ],
                None,
                None,
            )?;
            repo.revparse_single(rev)?
        }
    };
    // Peel tags/annotated tags down to the commit they name.
    let commit = object.peel(git2::ObjectType::Commit)?;
    let mut checkout = git2::build::CheckoutBuilder::new();
    checkout.force();
    repo.checkout_tree(&commit, Some(&mut checkout))?;
    repo.set_head_detached(commit.id())?;
    Ok(())
}

fn update_existing_repo(repo: &git2::Repository) -> Result<(), git2::Error> {
    let mut remote = repo.find_remote("origin")?;
    // `refs/heads/main:refs/remotes/origin/main` is the canonical refspec —
    // older libgit2 versions silently accept the bare `"main"` and newer ones
    // reject it as malformed OR, worse, interpret it as a write-back to the
    // local branch. Naming both ends keeps the fetch deterministic across
    // versions and pins the destination the reset below reads from.
    remote.fetch(&["refs/heads/main:refs/remotes/origin/main"], None, None)?;
    let fetch_head = repo.find_reference("FETCH_HEAD")?;
    let target = repo.reference_to_annotated_commit(&fetch_head)?.id();
    let obj = repo.find_object(target, None)?;
    // Canonical `reset --hard` — discard any local drift, match origin/main.
    repo.reset(&obj, git2::ResetType::Hard, None)?;
    if let Ok(mut head) = repo.find_reference("refs/heads/main") {
        let _ = head.set_target(target, "sync");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a tiny local git repo with one commit on `main` and return its path.
    fn make_source_repo(dir: &std::path::Path, content: &str) -> String {
        let repo = git2::Repository::init(dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), content).unwrap();
        let mut idx = repo.index().unwrap();
        idx.add_path(std::path::Path::new("SKILL.md")).unwrap();
        idx.write().unwrap();
        let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        repo.commit(Some("refs/heads/main"), &sig, &sig, "init", &tree, &[])
            .unwrap();
        repo.set_head("refs/heads/main").unwrap();
        dir.to_string_lossy().to_string()
    }

    #[test]
    fn clone_then_update_pulls_latest() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        let url = make_source_repo(&src, "v1");
        let checkout = tmp.path().join("checkout");

        clone_or_update(&url, &checkout).expect("clone");
        assert_eq!(
            std::fs::read_to_string(checkout.join("SKILL.md")).unwrap(),
            "v1"
        );

        // New commit upstream, then update → hard reset picks it up.
        let repo = git2::Repository::open(&src).unwrap();
        std::fs::write(src.join("SKILL.md"), "v2").unwrap();
        let mut idx = repo.index().unwrap();
        idx.add_path(std::path::Path::new("SKILL.md")).unwrap();
        idx.write().unwrap();
        let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        repo.commit(Some("refs/heads/main"), &sig, &sig, "v2", &tree, &[&parent])
            .unwrap();

        clone_or_update(&url, &checkout).expect("update");
        assert_eq!(
            std::fs::read_to_string(checkout.join("SKILL.md")).unwrap(),
            "v2"
        );
    }

    #[test]
    fn clone_unreachable_remote_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let checkout = tmp.path().join("checkout");
        let err = clone_or_update("/nonexistent/repo/path", &checkout);
        assert!(err.is_err());
    }

    /// A pinned revision materializes that exact tree — and stays pinned when the
    /// default branch moves on. Without this, a catalog entry declaring
    /// `git_ref` silently installed whatever HEAD happened to be.
    #[test]
    fn pinned_ref_checks_out_that_revision_and_survives_upstream_moves() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        let url = make_source_repo(&src, "v1");

        // Tag the first commit, then move `main` forward.
        let repo = git2::Repository::open(&src).unwrap();
        let first = repo.head().unwrap().peel_to_commit().unwrap();
        repo.tag_lightweight("v1.0.0", first.as_object(), false)
            .unwrap();
        std::fs::write(src.join("SKILL.md"), "v2").unwrap();
        let mut idx = repo.index().unwrap();
        idx.add_path(std::path::Path::new("SKILL.md")).unwrap();
        idx.write().unwrap();
        let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        repo.commit(Some("refs/heads/main"), &sig, &sig, "v2", &tree, &[&first])
            .unwrap();

        let checkout = tmp.path().join("pinned");
        clone_or_update_at(&url, &checkout, Some("v1.0.0")).expect("pinned clone");
        assert_eq!(
            std::fs::read_to_string(checkout.join("SKILL.md")).unwrap(),
            "v1",
            "pinned tag must not track main"
        );
        // Re-running the pin is idempotent and still does not advance to v2.
        clone_or_update_at(&url, &checkout, Some("v1.0.0")).expect("pinned re-check");
        assert_eq!(
            std::fs::read_to_string(checkout.join("SKILL.md")).unwrap(),
            "v1"
        );
        // A full commit SHA pins just as well.
        let by_sha = tmp.path().join("by-sha");
        clone_or_update_at(&url, &by_sha, Some(&first.id().to_string())).expect("sha clone");
        assert_eq!(
            std::fs::read_to_string(by_sha.join("SKILL.md")).unwrap(),
            "v1"
        );
    }

    #[test]
    fn unknown_pinned_ref_errors_rather_than_installing_head() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        let url = make_source_repo(&src, "v1");
        let checkout = tmp.path().join("checkout");
        let err = clone_or_update_at(&url, &checkout, Some("v9.9.9-nope"));
        assert!(err.is_err(), "an unresolvable pin must fail loudly");
    }
}
