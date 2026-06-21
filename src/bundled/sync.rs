//! Official-content git sync — clone the external skills/plugins repos into an
//! isolated checkout, or hard-reset an existing checkout to `origin/main`.
//! Uses git2 (libgit2, vendored) so no system `git` is required. Network I/O is
//! blocking — call from `spawn_blocking`. Never panics; returns `Err` so the
//! caller can fall back to the embedded snapshot.

use std::path::Path;
use tracing::info;

/// Clone `repo_url` (branch `main`) into `checkout_dir` if absent; otherwise
/// fetch and hard-reset the working tree to `origin/main`. The checkout dir is
/// official-content-only and never user-edited, so a hard reset is conflict-free.
pub(crate) fn clone_or_update(repo_url: &str, checkout_dir: &Path) -> Result<(), String> {
    if checkout_dir.join(".git").exists() {
        return update_existing(checkout_dir).map_err(|e| format!("update {repo_url}: {e}"));
    }
    if let Some(parent) = checkout_dir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    info!(url = %repo_url, dest = %checkout_dir.display(), "Cloning official content");
    git2::Repository::clone(repo_url, checkout_dir)
        .map(|_| ())
        .map_err(|e| format!("clone {repo_url}: {e}"))
}

fn update_existing(checkout_dir: &Path) -> Result<(), git2::Error> {
    let repo = git2::Repository::open(checkout_dir)?;
    let mut remote = repo.find_remote("origin")?;
    remote.fetch(&["main"], None, None)?;
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
        repo.commit(Some("refs/heads/main"), &sig, &sig, "init", &tree, &[]).unwrap();
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
        assert_eq!(std::fs::read_to_string(checkout.join("SKILL.md")).unwrap(), "v1");

        // New commit upstream, then update → hard reset picks it up.
        let repo = git2::Repository::open(&src).unwrap();
        std::fs::write(src.join("SKILL.md"), "v2").unwrap();
        let mut idx = repo.index().unwrap();
        idx.add_path(std::path::Path::new("SKILL.md")).unwrap();
        idx.write().unwrap();
        let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        repo.commit(Some("refs/heads/main"), &sig, &sig, "v2", &tree, &[&parent]).unwrap();

        clone_or_update(&url, &checkout).expect("update");
        assert_eq!(std::fs::read_to_string(checkout.join("SKILL.md")).unwrap(), "v2");
    }

    #[test]
    fn clone_unreachable_remote_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let checkout = tmp.path().join("checkout");
        let err = clone_or_update("/nonexistent/repo/path", &checkout);
        assert!(err.is_err());
    }
}
