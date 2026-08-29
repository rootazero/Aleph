//! Working-directory jail for `pty.spawn`.
//!
//! The client's `cwd` is a *request*, not an authorisation: the gate resolves
//! it against the operator-registered workspace roots
//! ([`super::workspace_roots`]) and refuses anything outside. `EXEC_WORKSPACE`
//! — the equivalent floor for the `bash` tool — is a `tokio::task_local`
//! scoped to an agent run and is structurally unreachable from an RPC
//! handler, which is why the source of truth here is the workspace-root
//! config lookup instead.
//!
//! This gate covers the *starting* directory only. A `cd` typed inside the
//! terminal is not constrained, because a command-grained gate is not
//! expressible on an interactive byte stream. What it buys is "every terminal
//! starts somewhere enumerable and auditable", not isolation.
//!
//! ## One direction, not two
//!
//! A deny-glob check (`sandbox::protected_paths`) needs both "am I inside a
//! protected area" and "is a protected area below me", because it guards an
//! operation that can recurse across an entire subtree in either direction.
//! This gate is a positive allowlist for a single starting directory, not a
//! blocklist: `asked.starts_with(root)` already rejects an `asked` that is an
//! *ancestor* of a root (e.g. `cwd: "/"` when the root is
//! `/Users/x/.aleph/workspaces`) because an ancestor is not "inside" — there
//! is no second relation left to check.
//!
//! ## Fails closed on every arm
//!
//! * a root that cannot be canonicalised (missing, unreadable, a dangling
//!   symlink) is dropped from the allowed set rather than erroring the whole
//!   call — a root going away must narrow what is allowed, never widen it;
//! * an empty roots list refuses loudly, naming the remedy, rather than
//!   picking a directory on the caller's behalf;
//! * a requested path that cannot be canonicalised (does not exist) is an
//!   error, never a fallback to some default — a PTY's starting directory
//!   must already exist, the same rule a plain `cd` follows;
//! * an omitted/blank `cwd` falls back to the first registered root, never to
//!   the daemon's process cwd — that answers "where was the server started",
//!   a different question, and a default that answers a different question
//!   is a lie dressed as a default.

use std::path::{Path, PathBuf};

/// Canonicalise one path. Both sides of the containment check below go
/// through this exact function — on Windows `canonicalize` yields the
/// `\\?\C:\` extended-length form, and canonicalising only one side of a
/// `starts_with` comparison is how it silently flips from allow to deny.
/// (`utils::paths::display_string` exists for the opposite boundary: turning
/// an already-canonical path back into a form fit for a human or an external
/// process to read, *after* every comparison is done — never before.)
fn canonical(p: &Path) -> Result<PathBuf, String> {
    std::fs::canonicalize(p).map_err(|e| format!("cannot resolve {}: {e}", p.display()))
}

/// Resolve the directory a new PTY may start in.
///
/// * `requested` — the client's ask, or `None`/blank to take the default.
/// * `roots` — the operator-registered workspace roots, read fresh by the
///   caller on every spawn (see [`super::workspace_roots`]) so a workspace
///   registered after start-up does not require a restart to become usable.
pub fn resolve_spawn_cwd(requested: Option<&str>, roots: &[PathBuf]) -> Result<PathBuf, String> {
    let canonical_roots: Vec<PathBuf> = roots.iter().filter_map(|r| canonical(r).ok()).collect();
    if canonical_roots.is_empty() {
        return Err(
            "no workspace is registered, so there is no directory a terminal may start in — \
             register one first (Panel → Settings → Workspaces, or `aleph workspace create`)"
                .to_string(),
        );
    }

    let Some(requested) = requested.filter(|s| !s.trim().is_empty()) else {
        // Not the daemon's cwd: that answers "where was the server started",
        // which is a different question from "what is this terminal
        // authorised to work in".
        return Ok(canonical_roots[0].clone());
    };

    let asked = canonical(Path::new(requested))?;
    if canonical_roots.iter().any(|root| asked.starts_with(root)) {
        Ok(asked)
    } else {
        Err(format!(
            "cwd {} is outside every registered workspace",
            asked.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        vec![dir.to_path_buf()]
    }

    #[test]
    fn a_path_inside_a_registered_root_is_allowed() {
        let tmp = tempfile::tempdir().expect("tmp");
        let sub = tmp.path().join("proj");
        std::fs::create_dir_all(&sub).expect("mkdir");
        let got = resolve_spawn_cwd(Some(sub.to_str().expect("utf8")), &roots(tmp.path()))
            .expect("inside a root must be allowed");
        assert!(got.ends_with("proj"));
    }

    #[test]
    fn a_path_outside_every_root_is_refused() {
        let tmp = tempfile::tempdir().expect("tmp");
        let other = tempfile::tempdir().expect("tmp2");
        let err = resolve_spawn_cwd(
            Some(other.path().to_str().expect("utf8")),
            &roots(tmp.path()),
        )
        .expect_err("outside every root must be refused");
        assert!(err.contains("outside"), "the refusal must say why: {err}");
    }

    /// The classic escape: a path that lexically looks inside but resolves out.
    #[test]
    fn dot_dot_traversal_is_refused_after_canonicalisation() {
        let tmp = tempfile::tempdir().expect("tmp");
        let sub = tmp.path().join("proj");
        std::fs::create_dir_all(&sub).expect("mkdir");
        let sneaky = sub.join("..").join("..");
        assert!(
            resolve_spawn_cwd(sneaky.to_str(), &roots(tmp.path())).is_err(),
            "traversal must be judged on the canonical form, not the literal one"
        );
    }

    /// Omitting cwd must not fall back to the daemon's process cwd: that
    /// answers a different question ("where was the server started") and
    /// would be a lie dressed as a default.
    #[test]
    fn an_omitted_cwd_falls_back_to_the_first_root_not_the_process_cwd() {
        let tmp = tempfile::tempdir().expect("tmp");
        let got = resolve_spawn_cwd(None, &roots(tmp.path())).expect("must resolve");
        let expected = std::fs::canonicalize(tmp.path()).expect("canonical");
        assert_eq!(got, expected);
        assert_ne!(
            got,
            std::env::current_dir().expect("cwd"),
            "the daemon's cwd is never the answer"
        );
    }

    /// With nothing registered the refusal must name the remedy, not pick a
    /// directory on the user's behalf.
    #[test]
    fn no_registered_roots_refuses_loudly_and_names_the_remedy() {
        let err = resolve_spawn_cwd(None, &[]).expect_err("must refuse");
        assert!(
            err.contains("workspace"),
            "the refusal must name what to do: {err}"
        );
    }
}
