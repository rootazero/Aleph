//! Per-project memory namespacing primitives.
//!
//! Aleph runs as a desktop App where the user works across many project
//! directories (Claude-Code style). Memories captured while working in project
//! A should not bleed into project B. The whole memory stack is already
//! partitioned by `agent_id` (PK on every table + the on-disk
//! `note/{agent_id}/…` layout), so per-project isolation is expressed by
//! *composing* the active project into that existing partition key rather than
//! adding a new schema dimension.
//!
//! This module is the single source of truth for that derivation. It is pure
//! (no I/O beyond canonicalization) so it can be unit-tested in isolation and
//! reused by every read/write seam.
//!
//! ## Invariants
//!
//! - **Gated, default-off.** With no active project (the common case, and the
//!   only case when `MemoryConfig.project_scoped` is false) the namespace is
//!   the sentinel [`GLOBAL_NS`] and [`scoped_agent_id`] returns the base id
//!   unchanged — byte-for-byte the pre-feature behaviour.
//! - **Stable.** The project hash uses SHA-256 (already a dependency) rather
//!   than `DefaultHasher`, whose `SipHash` output is not guaranteed stable across
//!   Rust versions. A persisted namespace key MUST be reproducible across
//!   restarts and compiler upgrades.
//! - **Floors 分床 (split, P1).** The two always-on floors no longer share one
//!   rule. The *user-profile* floor follows the session's personal scope —
//!   loaded under [`session_write_id`], so a personal session sees its own
//!   profile — while the *feedback/behaviour* floor stays under the *base* id
//!   unconditionally (org-wide standing rules apply to everyone). Callers get
//!   this by passing each floor loader the id `session_write_id`/the bare base
//!   id respectively — this module imposes nothing beyond exposing both.
//!
//! ## Session scope vs. the legacy project-directory feature
//!
//! [`crate::scope`] (P1) adds a second, independent scoping axis on top of the
//! project-directory feature this module originally shipped with. The two
//! compose through the SAME suffix mechanism ([`scoped_agent_id`]) but are
//! resolved differently: project-directory scoping is a config toggle
//! (`MemoryConfig.project_scoped`) that derives its suffix from the *active
//! project root* ([`project_namespace`]); session scoping reads the *ambient
//! task-local* ([`crate::scope::current_scope`]) set once per run/session and
//! is never gated by that config flag. [`session_write_id`]/[`session_read_ids`]
//! are the composition points that decide which axis wins for a given call:
//! **personal scope always wins over the project-directory feature** — a
//! personal session's memories are the user's own even while working inside a
//! project directory, so the two axes are siblings, never nested (see
//! [`crate::scope`]'s module doc: `proj-*` / `u-*` / `p-*` are sibling suffix
//! families). Every pre-P1 write/read seam that predates [`crate::scope`]
//! keeps calling [`scoped_or_base`]/[`read_scope_ids`] directly and is
//! unaffected; new session-scope-aware seams call `session_write_id`/
//! `session_read_ids` instead.

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::gateway::security::store::OWNER_USER_ID;
use crate::scope::{current_scope, ScopeId};

/// Sentinel namespace used when no project is active (or the feature is off).
pub const GLOBAL_NS: &str = "global";

/// Separator between the base `agent_id` and the project namespace inside a
/// composed id. Chosen because it never appears in an `agent_id` produced by
/// [`crate::routing::session_key::SessionKey`] (which uses `:` as its own field
/// separator) and is filesystem-safe on every supported platform.
const NS_SEP: &str = "__";

/// Derive the stable project namespace token for an optional project root.
///
/// Returns [`GLOBAL_NS`] when `project_root` is `None`. Otherwise returns
/// `"proj-<8 hex>"` where the hex is the first 4 bytes of the SHA-256 of the
/// canonicalised absolute path (falling back to the lossy path string if the
/// path cannot be canonicalised, e.g. it was deleted between resolution and
/// here). 8 hex chars (32 bits) is ample to separate the handful of projects a
/// single user touches while keeping on-disk directory names short.
#[must_use]
pub fn project_namespace(project_root: Option<&Path>) -> String {
    let Some(root) = project_root else {
        return GLOBAL_NS.to_string();
    };
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut hasher = Sha256::new();
    hasher.update(canonical.to_string_lossy().as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(8);
    for byte in &digest[..4] {
        hex.push_str(&format!("{byte:02x}"));
    }
    format!("proj-{hex}")
}

/// Returns `true` when `ns` is the global sentinel (no project / feature off).
#[must_use]
pub fn is_global(ns: &str) -> bool {
    ns == GLOBAL_NS
}

/// Compose the storage partition key for a memory write/read given the base
/// `agent_id` and a project namespace.
///
/// Global namespace → the base id unchanged (non-breaking). Otherwise the
/// project is appended with [`NS_SEP`] so the existing `(agent_id, …)` partition
/// — DB rows and the `note/{agent_id}/…` directory alike — isolates the project
/// automatically with no schema change.
#[must_use]
pub fn scoped_agent_id(base: &str, ns: &str) -> String {
    if is_global(ns) {
        base.to_string()
    } else {
        format!("{base}{NS_SEP}{ns}")
    }
}

/// The set of partition keys to query when *reading* memory in a given project.
///
/// In a project we want the project's own memories **plus** the agent's global
/// (cross-project) knowledge, so reads union `[base, scoped]`. Outside any
/// project (global namespace) reads see only the base id. The existing
/// `NoteFactRetrieval::retrieve_multi_agent` consumes exactly this list, so the
/// read side needs no new query machinery.
#[must_use]
pub fn read_scope_ids(base: &str, ns: &str) -> Vec<String> {
    if is_global(ns) {
        vec![base.to_string()]
    } else {
        vec![base.to_string(), scoped_agent_id(base, ns)]
    }
}

/// Resolve the storage agent id for a memory write given the base agent id,
/// whether project scoping is enabled, and the active project root.
///
/// This is the single composition chokepoint shared by every write seam (the
/// `note_manage` tool and post-turn session compaction). With scoping off it
/// returns the base id unchanged — byte-for-byte the pre-feature behaviour —
/// so callers can route every write through it unconditionally.
#[must_use]
pub fn scoped_or_base(base: &str, project_scoped: bool, project_root: Option<&Path>) -> String {
    if project_scoped {
        scoped_agent_id(base, &project_namespace(project_root))
    } else {
        base.to_string()
    }
}

/// Sibling suffix families a composed agent id can carry (see [`crate::scope`]
/// module doc): `proj-*` is the legacy project-directory feature, `u-*` is
/// personal scope, `p-*` is project scope (P2). [`list_scoped_agent_ids`]
/// scans for all three so dream-daemon maintenance never silently skips a
/// family — only the union of exactly these three is ever recognised as a
/// scoped sibling directory.
const SCOPED_FAMILIES: [&str; 3] = ["proj-", "u-", "p-"];

/// Enumerate the scoped composed agent ids that already have memory on disk
/// for a given base agent, across every sibling suffix family
/// ([`SCOPED_FAMILIES`]).
///
/// The note store lays memory out as `note/{agent_id}/…`, so the scoped
/// namespaces created by [`scoped_agent_id`] surface as sibling directories
/// named `{base}__<family>-<ref>`. This scans `memory_dir` and returns exactly
/// those names (sorted for deterministic iteration), letting the dream daemon
/// fan its per-namespace maintenance over every namespace that actually has
/// notes — a namespace the user never wrote a note in needs no maintenance,
/// and the base directory itself is intentionally excluded (the caller
/// maintains the base separately). Returns an empty vec when the dir is
/// absent or unreadable, so an off / fresh install is a clean no-op.
#[must_use]
pub fn list_scoped_agent_ids(memory_dir: &Path, base: &str) -> Vec<String> {
    let prefix = format!("{base}{NS_SEP}");
    let Ok(entries) = std::fs::read_dir(memory_dir) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| {
            name.strip_prefix(&prefix)
                .is_some_and(|ns| SCOPED_FAMILIES.iter().any(|fam| ns.starts_with(fam)))
        })
        .collect();
    ids.sort();
    ids
}

/// Resolve the storage agent id for a memory WRITE within the *current
/// session's* scope (spec §5.2). This is [`scoped_or_base`]'s session-scope-
/// aware sibling: `Personal(u)` wins over the legacy project-directory
/// feature unconditionally — a personal session's writes are always the
/// user's own, even while working inside a project directory — because the
/// two suffix families are siblings, never nested (see module doc). With no
/// active personal scope (`Org`, `Project`, or no scope at all) this is
/// byte-for-byte [`scoped_or_base`], so every pre-P1 caller is unaffected.
#[must_use]
pub fn session_write_id(base: &str, project_scoped: bool, project_root: Option<&Path>) -> String {
    match current_scope().map(|attr| attr.scope) {
        Some(ScopeId::Personal(ref_id)) => scoped_agent_id(base, &ref_id),
        _ => scoped_or_base(base, project_scoped, project_root),
    }
}

/// The set of partition keys to query when *reading* memory within the
/// current session's scope (spec §5.2). `Personal(u)` unions `[org,
/// personal:u]` — org-first, the same order contract [`read_scope_ids`]
/// already has — regardless of `project_scoped`: personal scope is never
/// gated by the legacy project-directory toggle. With no active personal
/// scope this falls back to [`read_scope_ids`] via [`project_namespace`],
/// i.e. today's behaviour, unchanged.
#[must_use]
pub fn session_read_ids(
    base: &str,
    project_scoped: bool,
    project_root: Option<&Path>,
) -> Vec<String> {
    match current_scope().map(|attr| attr.scope) {
        Some(ScopeId::Personal(ref_id)) => {
            vec![base.to_string(), scoped_agent_id(base, &ref_id)]
        }
        _ => {
            if project_scoped {
                read_scope_ids(base, &project_namespace(project_root))
            } else {
                vec![base.to_string()]
            }
        }
    }
}

/// If `agent_id` is the composed personal-scope id for the single-machine
/// owner (`{base}__u-owner`, i.e. `scoped_agent_id(base, OWNER_USER_ID)`),
/// returns the bare `base` it should one-time-adopt pre-P1 curated content
/// from. `None` for every other shape — base ids, `proj-`/`p-` composed ids,
/// and any *other* user's personal scope never adopt: only the owner could
/// have pre-existing single-user content to inherit.
#[must_use]
pub fn owner_adoption_base(agent_id: &str) -> Option<&str> {
    agent_id.strip_suffix(&format!("{NS_SEP}{OWNER_USER_ID}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn no_project_is_global_sentinel() {
        assert_eq!(project_namespace(None), GLOBAL_NS);
        assert!(is_global(&project_namespace(None)));
    }

    #[test]
    fn project_namespace_is_stable_and_prefixed() {
        let dir = tempdir().unwrap();
        let a = project_namespace(Some(dir.path()));
        let b = project_namespace(Some(dir.path()));
        assert_eq!(a, b, "same path must hash identically across calls");
        assert!(a.starts_with("proj-"), "got {a}");
        assert_eq!(a.len(), "proj-".len() + 8, "8 hex chars: {a}");
        assert!(!is_global(&a));
    }

    #[test]
    fn distinct_projects_get_distinct_namespaces() {
        let a = tempdir().unwrap();
        let b = tempdir().unwrap();
        assert_ne!(
            project_namespace(Some(a.path())),
            project_namespace(Some(b.path()))
        );
    }

    #[test]
    fn uncanonicalizable_path_still_derives_stably() {
        // A path that does not exist cannot be canonicalised; the lossy
        // fallback must still be deterministic.
        let missing = PathBuf::from("/no/such/aleph/project/xyz");
        let a = project_namespace(Some(&missing));
        let b = project_namespace(Some(&missing));
        assert_eq!(a, b);
        assert!(a.starts_with("proj-"));
    }

    #[test]
    fn scoped_agent_id_is_identity_when_global() {
        assert_eq!(scoped_agent_id("main", GLOBAL_NS), "main");
    }

    #[test]
    fn scoped_agent_id_composes_with_separator() {
        let scoped = scoped_agent_id("main", "proj-deadbeef");
        assert_eq!(scoped, "main__proj-deadbeef");
        // Must not collide with a session-key field separator.
        assert!(!scoped.contains(':'));
    }

    #[test]
    fn read_scope_ids_global_is_base_only() {
        assert_eq!(read_scope_ids("main", GLOBAL_NS), vec!["main".to_string()]);
    }

    #[test]
    fn read_scope_ids_in_project_unions_base_and_scoped() {
        let ids = read_scope_ids("main", "proj-deadbeef");
        assert_eq!(
            ids,
            vec!["main".to_string(), "main__proj-deadbeef".to_string()]
        );
    }

    #[test]
    fn scoped_or_base_off_is_identity_regardless_of_project() {
        let dir = tempdir().unwrap();
        assert_eq!(scoped_or_base("main", false, Some(dir.path())), "main");
        assert_eq!(scoped_or_base("main", false, None), "main");
    }

    #[test]
    fn scoped_or_base_on_composes_only_with_active_project() {
        // On + no project → base (global namespace collapses to base).
        assert_eq!(scoped_or_base("main", true, None), "main");
        // On + project → composed scoped id.
        let dir = tempdir().unwrap();
        let scoped = scoped_or_base("main", true, Some(dir.path()));
        assert!(scoped.starts_with("main__proj-"), "got {scoped}");
    }

    #[test]
    fn list_scoped_agent_ids_missing_dir_is_empty() {
        let missing = PathBuf::from("/no/such/aleph/memory/dir");
        assert!(list_scoped_agent_ids(&missing, "main").is_empty());
    }

    #[test]
    fn list_scoped_agent_ids_returns_only_project_dirs_for_base() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        // Project namespaces for the base agent — should be returned.
        std::fs::create_dir(root.join("main__proj-aaaaaaaa")).unwrap();
        std::fs::create_dir(root.join("main__proj-bbbbbbbb")).unwrap();
        // Base agent dir itself — excluded (maintained separately).
        std::fs::create_dir(root.join("main")).unwrap();
        // A different base agent's project — excluded (wrong base).
        std::fs::create_dir(root.join("other__proj-cccccccc")).unwrap();
        // A stray file with the right prefix — excluded (not a dir).
        std::fs::write(root.join("main__proj-notadir"), b"x").unwrap();

        let ids = list_scoped_agent_ids(root, "main");
        assert_eq!(
            ids,
            vec![
                "main__proj-aaaaaaaa".to_string(),
                "main__proj-bbbbbbbb".to_string()
            ]
        );
    }

    #[test]
    fn dream_scan_lists_personal_and_project_dirs() {
        // P1: the dream daemon's per-namespace fan-out must not silently skip
        // personal-scope directories — only the un-prefixed junk is excluded.
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("main__proj-x")).unwrap();
        std::fs::create_dir(root.join("main__u-a")).unwrap();
        std::fs::create_dir(root.join("main__junk")).unwrap();

        let ids = list_scoped_agent_ids(root, "main");
        assert_eq!(
            ids,
            vec!["main__proj-x".to_string(), "main__u-a".to_string()]
        );
    }

    #[tokio::test]
    async fn personal_scope_wins_the_write_id() {
        crate::scope::with_scope(
            Some(crate::scope::ScopeAttribution::personal("u-alice")),
            async {
                assert_eq!(
                    session_write_id("main", true, Some(Path::new("/repo"))),
                    "main__u-alice"
                );
            },
        )
        .await;
    }

    #[tokio::test]
    async fn unscoped_write_id_is_byte_identical_to_scoped_or_base() {
        // No task-local scope: session_write_id must match scoped_or_base
        // exactly, for both the off and the project-active configurations —
        // the single-user zero-change pin.
        let dir = tempdir().unwrap();
        for (project_scoped, root) in [(false, None), (true, Some(dir.path()))] {
            assert_eq!(
                session_write_id("main", project_scoped, root),
                scoped_or_base("main", project_scoped, root),
                "project_scoped={project_scoped}"
            );
        }
    }

    #[tokio::test]
    async fn personal_read_union_is_org_then_personal() {
        crate::scope::with_scope(
            Some(crate::scope::ScopeAttribution::personal("u-alice")),
            async {
                assert_eq!(
                    session_read_ids("main", false, None),
                    vec!["main".to_string(), "main__u-alice".to_string()]
                );
            },
        )
        .await;
    }

    #[tokio::test]
    async fn unscoped_read_ids_are_byte_identical_to_read_scope_ids() {
        let dir = tempdir().unwrap();
        assert_eq!(
            session_read_ids("main", false, None),
            vec!["main".to_string()]
        );
        assert_eq!(
            session_read_ids("main", true, Some(dir.path())),
            read_scope_ids("main", &project_namespace(Some(dir.path())))
        );
    }

    #[test]
    fn owner_adoption_base_matches_only_the_owners_composed_id() {
        assert_eq!(owner_adoption_base("main__u-owner"), Some("main"));
        assert_eq!(owner_adoption_base("main__u-alice"), None);
        assert_eq!(owner_adoption_base("main"), None);
        assert_eq!(owner_adoption_base("main__proj-deadbeef"), None);
    }
}
