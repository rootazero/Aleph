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
//! - **Floors 分床 (split, P1/P2).** The two always-on floors do not share one
//!   rule. The *user-profile* floor follows the session's personal scope and is
//!   ABSENT in a shared room — [`profile_floor_id`] is its single source, and it
//!   returns `Option` precisely so "there is no such thing here" is expressible.
//!   The *feedback/behaviour* floor is **not narrowed** to the personal
//!   partition, but neither is it pinned to the base id: it reads
//!   [`session_read_ids`], i.e. base ∪ this session's own partition, the same
//!   set ordinary note retrieval uses. Org-wide standing rules therefore reach
//!   everyone (base is always in that set) *and* a correction the user just
//!   made reaches them.
//!
//!   It used to be documented — and implemented — as "base id, unconditionally".
//!   That could not work: every writer goes through
//!   `caller_memory_partition`/[`session_write_id`], and a zero-config loopback
//!   Panel session is already `Personal(u-owner)`, so the factory-default write
//!   lands in `main__u-owner/feedback/` while the floor scanned `main/feedback/`.
//!   The always-on leg was empty out of the box, silently, because those rules
//!   still surfaced whenever retrieval matched them lexically.
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
///
/// `pub(crate)` so [`crate::gateway::visibility::partition_visible`] (P1) can
/// split a composed id on the SAME literal rather than re-declaring it — two
/// separators for one grammar is exactly the kind of drift this module's own
/// doc warns about.
pub(crate) const NS_SEP: &str = "__";

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

/// `true` when `agent_id` already carries a scope suffix from one of
/// [`SCOPED_FAMILIES`], i.e. it is the OUTPUT of a composition rather than a
/// base agent id.
///
/// Exists because composition is **not idempotent**: feeding an already
/// composed id back through [`session_write_id`] under an active scope yields
/// `main__u-bob__u-bob`, a partition nobody writes and everybody's reads
/// miss. Callers that accept an agent id off the wire and then let the
/// resolver compose it (the `memory.curated.*` handlers; the builtin tool
/// registry's `remember` arm carries the same warning in prose) use this to
/// refuse a pre-composed id instead of silently landing on a phantom store.
///
/// Derived from [`SCOPED_FAMILIES`], never from a second literal list: a new
/// family added there is recognised here for free.
#[must_use]
pub fn is_composed_id(agent_id: &str) -> bool {
    agent_id
        .split_once(NS_SEP)
        .is_some_and(|(_, suffix)| SCOPED_FAMILIES.iter().any(|f| suffix.starts_with(f)))
}

/// Enumerate every note **corpus** that has memory on disk under `memory_dir`.
///
/// A corpus is one `note/{agent_id}/` directory — the partition key every note
/// table is keyed by. A base agent id and each composed scoped id
/// (`{base}__u-…`, `{base}__p-…`, `{base}__proj-…`) are corpora in exactly the
/// same sense: they differ only in how the id was composed, never in how the
/// notes underneath are stored, indexed, embedded or maintained. Anything that
/// maintains "all of memory" therefore has to iterate this, not just the
/// default agent.
///
/// **This is the single source for "which corpora exist".** It used to be
/// answered three times — `note_retrieval::discover_agent_ids` (skipped
/// dot-directories), `reembed::discover_agent_ids` (did not, so a stray
/// `.something` was re-embedded as if it were an agent), and
/// [`list_scoped_agent_ids`] (only the siblings of one base). Three answers to
/// one question is how a maintenance pass silently covers a different set than
/// the pass next to it.
///
/// Dot-prefixed directories are excluded: the note tree carries editor and
/// staging state (`.obsidian/` per agent, atomic-write temp files) and none of
/// it is a partition. Returns an empty vec when the dir is absent or unreadable
/// — a fresh install is a clean no-op, and this is a *sensor*: it never creates
/// the directory it measures.
#[must_use]
pub fn list_note_corpora(memory_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(memory_dir) else {
        return Vec::new();
    };
    // Distinguish per-entry errors (a permissions error on a single sub-corpus)
    // from a clean walk. The previous `flatten()` silently dropped both, so a
    // vault with a partial permissions issue returned a partial list and the
    // dream fan-out silently skipped the un-listed corpora.
    let mut ids: Vec<String> = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    dir = %memory_dir.display(),
                    "list_note_corpora: read_dir entry failed; skipping"
                );
                continue;
            }
        };
        let is_dir = match entry.file_type() {
            Ok(t) => t.is_dir(),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    entry = %entry.path().display(),
                    "list_note_corpora: file_type failed; skipping"
                );
                continue;
            }
        };
        if !is_dir {
            continue;
        }
        let name = match entry.file_name().into_string() {
            Ok(s) => s,
            Err(_) => {
                tracing::warn!(
                    entry = %entry.path().display(),
                    "list_note_corpora: non-UTF8 entry name; skipping"
                );
                continue;
            }
        };
        if name.is_empty() || name.starts_with('.') {
            continue;
        }
        ids.push(name);
    }
    ids.sort();
    ids.dedup();
    ids
}

/// Enumerate the scoped composed agent ids that already have memory on disk
/// for a given base agent, across every sibling suffix family
/// ([`SCOPED_FAMILIES`]).
///
/// The note store lays memory out as `note/{agent_id}/…`, so the scoped
/// namespaces created by [`scoped_agent_id`] surface as sibling directories
/// named `{base}__<family>-<ref>`. This filters [`list_note_corpora`] down to
/// exactly those names (sorted for deterministic iteration), letting the dream
/// daemon fan its per-namespace maintenance over every namespace that actually
/// has notes — a namespace the user never wrote a note in needs no maintenance,
/// and the base directory itself is intentionally excluded (the caller
/// maintains the base separately). Returns an empty vec when the dir is
/// absent or unreadable, so an off / fresh install is a clean no-op.
#[must_use]
pub fn list_scoped_agent_ids(memory_dir: &Path, base: &str) -> Vec<String> {
    let prefix = format!("{base}{NS_SEP}");
    list_note_corpora(memory_dir)
        .into_iter()
        .filter(|name| {
            name.strip_prefix(&prefix)
                .is_some_and(|ns| SCOPED_FAMILIES.iter().any(|fam| ns.starts_with(fam)))
        })
        .collect()
}

/// Resolve the storage agent id for a memory WRITE within the *current
/// session's* scope (spec §5.2, §13). This is [`scoped_or_base`]'s
/// session-scope-aware sibling, with one arm per [`ScopeId`]:
///
/// - `Personal(u)` → the user's own partition. Wins over the legacy
///   project-directory feature unconditionally — a personal session's writes
///   are always the user's own, even while working inside a project directory.
/// - `Project(p)` → the ROOM's partition (P2). This is what "项目记忆共享"
///   means mechanically: every member's turns in the room write to one
///   partition, so what one member taught the agent, the next member's turn
///   recalls. Also wins over the project-directory feature, for the same
///   reason `Personal` does — the three suffix families are siblings, never
///   nested (see module doc).
/// - `Org` / no scope at all → byte-for-byte [`scoped_or_base`], so every
///   pre-P1 caller is unaffected.
#[must_use]
pub fn session_write_id(base: &str, project_scoped: bool, project_root: Option<&Path>) -> String {
    match current_scope().map(|attr| attr.scope) {
        Some(ScopeId::Personal(ref_id)) | Some(ScopeId::Project(ref_id)) => {
            scoped_agent_id(base, &ref_id)
        }
        _ => scoped_or_base(base, project_scoped, project_root),
    }
}

/// The set of partition keys to query when *reading* memory within the
/// current session's scope (spec §5.2, §13). Mirrors [`session_write_id`]
/// arm for arm: a scoped session unions `[org, its own partition]` —
/// org-first, the same order contract [`read_scope_ids`] already has —
/// regardless of `project_scoped`, because session scope is never gated by
/// the legacy project-directory toggle. With no active session scope this
/// falls back to [`read_scope_ids`] via [`project_namespace`], i.e. today's
/// behaviour, unchanged.
///
/// **The union has exactly two members, and that is the isolation** (spec
/// §13): a project room recalls the org tier and its own room, and NOBODY's
/// personal partition — not even the creator's. Adding a third member here
/// would be a cross-user leak with no error anywhere, which is why
/// `a_project_room_recalls_nobodys_personal_memory` asserts on the shape of
/// the whole vec rather than on the two ids it expects to find.
#[must_use]
pub fn session_read_ids(
    base: &str,
    project_scoped: bool,
    project_root: Option<&Path>,
) -> Vec<String> {
    match current_scope().map(|attr| attr.scope) {
        Some(ScopeId::Personal(ref_id)) | Some(ScopeId::Project(ref_id)) => {
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

/// The partition whose user profile may be injected as THIS session's
/// always-on profile floor — or `None` when no profile is admissible at all.
///
/// `Option` is the whole point of this function existing beside
/// [`session_write_id`] rather than callers reusing it: in a project room the
/// answer is not "some other partition's profile", it is **there is no such
/// thing here**. A room is several people; "whose USER.md" has no answer, and
/// injecting the creator's is a pure leak that nothing would report.
///
/// The criterion, stated once so the next reader does not have to re-derive it:
/// **「有没有 scope」是结构性恒真，「这份 profile 属于房间里的谁」才是谓词。**
/// A gate written as `current_scope().is_some()` admits every project room.
///
/// - `Personal(u)` → `Some` the user's own partition.
/// - `Project(_)` → `None`. See above.
/// - `Org` / no scope → `Some` [`scoped_or_base`], i.e. pre-P1 behaviour
///   byte-for-byte (the single-user zero-change guarantee).
///
/// The write-side twin is [`partition_is_shared_room`], which answers the same
/// question from a composed id for callers that run with no ambient scope.
#[must_use]
pub fn profile_floor_id(
    base: &str,
    project_scoped: bool,
    project_root: Option<&Path>,
) -> Option<String> {
    match current_scope().map(|attr| attr.scope) {
        Some(ScopeId::Personal(ref_id)) => Some(scoped_agent_id(base, &ref_id)),
        Some(ScopeId::Project(_)) => None,
        _ => Some(scoped_or_base(base, project_scoped, project_root)),
    }
}

/// Does this composed partition id belong to a room with more than one person
/// in it — i.e. is there no single human whose profile it could describe?
///
/// The id-shaped twin of [`profile_floor_id`], for callers that run with **no
/// ambient scope live**: the compression scheduler fans out over
/// `SELECT DISTINCT agent_id FROM raw_memories` on a background task, long
/// after the run that produced those rows finished, so it has the partition id
/// and nothing else. Splits on the SAME literal and in the same direction as
/// [`crate::gateway::visibility::partition_visible`] — one grammar for one id
/// format.
///
/// `true` only for the `p-*` (project room) family. Note `"proj-…"` does NOT
/// start with `"p-"`, so the legacy project-DIRECTORY family keeps its
/// single-person ruling: a directory namespace on a single-user box still has
/// exactly one human behind it.
#[must_use]
pub fn partition_is_shared_room(agent_id: &str) -> bool {
    agent_id
        .split_once(NS_SEP)
        .is_some_and(|(_base, suffix)| suffix.starts_with("p-"))
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

/// Copy a pre-P1 base-partition file into the owner's personal-scope path,
/// publishing it atomically.
///
/// The single implementation shared by the two adoption hooks
/// (`MemoryContextProvider::adopt_owner_curated_file` for MEMORY.md /
/// OPEN_LOOPS.md, `FsProfileSynthesizer::adopt_owner_profile` for USER.md),
/// which had drifted into two near-identical copies of the same body. Both
/// gate on "the scoped file does not exist yet", so a half-written
/// destination would permanently suppress re-adoption — hence the copy lands
/// on a temp sibling and is `rename`d into place, which is atomic on every
/// supported platform. A leftover temp file from a crash is overwritten by
/// the next attempt (`fs::copy` truncates).
///
/// It COPIES: the source is the org-tier instance that every unscoped
/// principal reads, and moving it out from under them is the P1 defect this
/// exists to avoid. See `adopt_owner_curated_file`'s doc for the full
/// argument.
pub async fn copy_adopted_file(
    from: &Path,
    to: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut staging = to.as_os_str().to_owned();
    staging.push(".adopting");
    let staging = std::path::PathBuf::from(staging);

    if let Err(e) = tokio::fs::copy(from, &staging).await {
        let _ = tokio::fs::remove_file(&staging).await;
        return Err(Box::new(e));
    }
    if let Err(e) = tokio::fs::rename(&staging, to).await {
        let _ = tokio::fs::remove_file(&staging).await;
        return Err(Box::new(e));
    }
    Ok(())
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
    fn list_note_corpora_returns_every_partition_and_nothing_else() {
        // The single answer to "which corpora exist". Base agents and composed
        // scoped ids are corpora in the same sense; editor/staging state is not.
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("main")).unwrap();
        std::fs::create_dir(root.join("main__proj-aaaa")).unwrap();
        std::fs::create_dir(root.join("main__u-owner")).unwrap();
        std::fs::create_dir(root.join("researcher")).unwrap();
        // Not partitions: a dot-directory (the shape `.obsidian`/staging dirs
        // take) and a plain file.
        std::fs::create_dir(root.join(".staging")).unwrap();
        std::fs::write(root.join("README.md"), b"x").unwrap();

        assert_eq!(
            list_note_corpora(root),
            vec![
                "main".to_string(),
                "main__proj-aaaa".to_string(),
                "main__u-owner".to_string(),
                "researcher".to_string(),
            ]
        );
    }

    #[test]
    fn list_note_corpora_missing_dir_is_empty_and_creates_nothing() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("note");
        assert!(list_note_corpora(&missing).is_empty());
        assert!(
            !missing.exists(),
            "a sensor must never create the directory it measures"
        );
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

    #[tokio::test]
    async fn a_project_session_writes_to_the_room_partition() {
        // P2 §13: the room's partition is what "项目记忆共享" means
        // mechanically — and it beats the legacy project-DIRECTORY feature,
        // exactly as personal scope does.
        crate::scope::with_scope(
            Some(crate::scope::ScopeAttribution {
                owner_user_id: "u-alice".into(),
                scope: ScopeId::Project("p-x7f2".into()),
            }),
            async {
                assert_eq!(
                    session_write_id("main", true, Some(Path::new("/repo"))),
                    "main__p-x7f2"
                );
            },
        )
        .await;
    }

    #[tokio::test]
    async fn a_project_room_recalls_nobodys_personal_memory() {
        // The assertion is on the WHOLE vec, not on "the two ids I expect are
        // present": a third member sneaking in here is a cross-user leak that
        // reports nothing, so the shape is the invariant.
        crate::scope::with_scope(
            Some(crate::scope::ScopeAttribution {
                owner_user_id: "u-alice".into(),
                scope: ScopeId::Project("p-x7f2".into()),
            }),
            async {
                let ids = session_read_ids("main", false, None);
                assert_eq!(ids, vec!["main".to_string(), "main__p-x7f2".to_string()]);
                assert!(
                    !ids.iter().any(|id| id.contains("__u-")),
                    "spec §5.2/§13: a project room must NOT recall anyone's \
                     personal memory, not even its creator's: {ids:?}"
                );
            },
        )
        .await;
    }

    #[tokio::test]
    async fn two_members_of_one_room_share_a_partition() {
        // The acceptance criterion for P2, reduced to its storage form: the
        // same room resolves identically no matter which member is speaking.
        let mut resolved = Vec::new();
        for member in ["u-alice", "u-bob"] {
            resolved.push(
                crate::scope::with_scope(
                    Some(crate::scope::ScopeAttribution {
                        owner_user_id: member.into(),
                        scope: ScopeId::Project("p-x7f2".into()),
                    }),
                    async { session_write_id("main", false, None) },
                )
                .await,
            );
        }
        assert_eq!(resolved[0], resolved[1], "{resolved:?}");
        assert_eq!(resolved[0], "main__p-x7f2");
    }

    #[tokio::test]
    async fn the_profile_floor_is_absent_in_a_shared_room() {
        // Not "some other partition's profile" — none. See profile_floor_id's
        // doc: 「有没有 scope」是结构性恒真.
        crate::scope::with_scope(
            Some(crate::scope::ScopeAttribution {
                owner_user_id: "u-alice".into(),
                scope: ScopeId::Project("p-x7f2".into()),
            }),
            async {
                assert_eq!(
                    profile_floor_id("main", true, Some(Path::new("/repo"))),
                    None
                );
            },
        )
        .await;
    }

    #[tokio::test]
    async fn the_profile_floor_follows_personal_scope_and_is_unchanged_otherwise() {
        crate::scope::with_scope(
            Some(crate::scope::ScopeAttribution::personal("u-alice")),
            async {
                assert_eq!(
                    profile_floor_id("main", false, None).as_deref(),
                    Some("main__u-alice")
                );
            },
        )
        .await;
        // Unscoped: byte-identical to the pre-P2 call site, which passed
        // session_write_id's result straight through.
        let dir = tempdir().unwrap();
        for (project_scoped, root) in [(false, None), (true, Some(dir.path()))] {
            assert_eq!(
                profile_floor_id("main", project_scoped, root),
                Some(scoped_or_base("main", project_scoped, root)),
                "project_scoped={project_scoped}"
            );
        }
    }

    #[test]
    fn only_the_project_room_family_reads_as_shared() {
        assert!(partition_is_shared_room("main__p-x7f2"));
        // A bare base id, a personal partition, and the legacy project
        // DIRECTORY family all have exactly one human behind them. `proj-`
        // must not be swept up by the `p-` arm.
        assert!(!partition_is_shared_room("main"));
        assert!(!partition_is_shared_room("main__u-alice"));
        assert!(!partition_is_shared_room("main__proj-deadbeef"));
    }

    #[test]
    fn owner_adoption_base_matches_only_the_owners_composed_id() {
        assert_eq!(owner_adoption_base("main__u-owner"), Some("main"));
        assert_eq!(owner_adoption_base("main__u-alice"), None);
        assert_eq!(owner_adoption_base("main"), None);
        assert_eq!(owner_adoption_base("main__proj-deadbeef"), None);
    }
}
