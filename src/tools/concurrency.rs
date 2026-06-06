//! Resource-scope-aware concurrency claims for parallel tool dispatch.
//!
//! The harness parallel fast path ([`crate::harness::agent`]) needs to decide,
//! for a batch of tool calls the LLM emitted in one turn, whether they can run
//! concurrently without observing each other's side effects. Historically that
//! decision was a single per-tool boolean (`concurrent_safe`): a batch was
//! eligible only when EVERY call self-reported safe. That binary model is both
//!
//! * **too conservative** — two `file_ops` moves to *disjoint* paths, or two
//!   read-only `file_ops` listings, were forced serial purely because the tool
//!   is "exclusive"; and
//! * **too permissive** — `file_write` / `file_edit` / `apply_patch` are not
//!   on the exclusive list, so two concurrent edits to the *same file* could
//!   race.
//!
//! This module replaces the boolean with a three-state claim that names the
//! *blast radius* of a call. Conflict detection is then a pure, mechanical
//! resource-overlap check (a data-race guard, not an LLM judgement — so it
//! stays inside R7/R10: no intent inference, no relevance scoring, no tool
//! filtering; the LLM still chose every call, this only schedules them).
//!
//! Compared to hermes-agent's `_paths_overlap` (string-prefix heuristic) and
//! codex's `supports_parallel` (per-tool boolean), the claim model is *sound*:
//! overlap is decided by normalized path-component containment, so a write to
//! `src/a` correctly conflicts with a write to `src/a/b.rs` (nested), while
//! `src/a.rs` and `src/ab.rs` (sibling prefixes) do not.

use std::collections::BTreeSet;

/// What a single tool call needs in order to run safely alongside the other
/// calls in the same parallel batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConcurrencyClaim {
    /// Read-only / side-effect-free for scheduling purposes. Any number of
    /// `Shared` calls may run concurrently with each other. A `Shared` call
    /// still conflicts with any [`ConcurrencyClaim::Exclusive`] call because
    /// we cannot prove the read footprint is disjoint from the write.
    Shared,
    /// Mutating. Conflicts with other calls according to its [`ExclusiveScope`].
    Exclusive { scope: ExclusiveScope },
}

/// The blast radius of an exclusive (mutating) call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExclusiveScope {
    /// Touches an unbounded / unknown set of resources (e.g. `bash`, which can
    /// read or write anything). Conflicts with every other claim, forcing the
    /// batch onto the serial path. This is the conservative default a tool
    /// falls back to when no concrete resource set can be extracted.
    Global,
    /// Touches exactly this set of normalized filesystem paths. Two
    /// `Paths` scopes conflict only when their path sets overlap
    /// (equal or ancestor/descendant), so disjoint-path mutations parallelize.
    Paths(BTreeSet<String>),
}

impl ConcurrencyClaim {
    /// Convenience constructor for a bounded path scope from any path iterator.
    /// Non-empty paths are lexically normalized; empties are dropped. If the
    /// resulting set is empty the scope degrades to [`ExclusiveScope::Global`]
    /// (we could not pin a concrete footprint, so assume the worst).
    pub fn paths<I, S>(paths: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let set: BTreeSet<String> = paths
            .into_iter()
            .filter_map(|p| normalize_path(p.as_ref()))
            .collect();
        if set.is_empty() {
            ConcurrencyClaim::Exclusive {
                scope: ExclusiveScope::Global,
            }
        } else {
            ConcurrencyClaim::Exclusive {
                scope: ExclusiveScope::Paths(set),
            }
        }
    }

    /// The whole-world exclusive claim. Conflicts with everything.
    pub fn global() -> Self {
        ConcurrencyClaim::Exclusive {
            scope: ExclusiveScope::Global,
        }
    }
}

/// Whether two claims conflict — i.e. they must NOT run concurrently.
///
/// Truth table:
/// * `Shared`   vs `Shared`           → no conflict (reads parallelize)
/// * `Shared`   vs `Exclusive(_)`     → conflict (read may observe a torn write)
/// * `Global`   vs anything           → conflict (unbounded footprint)
/// * `Paths(a)` vs `Paths(b)`         → conflict iff the path sets overlap
pub fn claims_conflict(a: &ConcurrencyClaim, b: &ConcurrencyClaim) -> bool {
    use ConcurrencyClaim::{Exclusive, Shared};
    match (a, b) {
        (Shared, Shared) => false,
        (Shared, Exclusive { .. }) | (Exclusive { .. }, Shared) => true,
        (Exclusive { scope: sa }, Exclusive { scope: sb }) => scopes_conflict(sa, sb),
    }
}

fn scopes_conflict(a: &ExclusiveScope, b: &ExclusiveScope) -> bool {
    use ExclusiveScope::{Global, Paths};
    match (a, b) {
        (Global, _) | (_, Global) => true,
        (Paths(pa), Paths(pb)) => paths_overlap(pa, pb),
    }
}

/// Whether any path in `a` overlaps any path in `b`. Two paths overlap when
/// they are equal or one is a (component-wise) ancestor of the other.
fn paths_overlap(a: &BTreeSet<String>, b: &BTreeSet<String>) -> bool {
    a.iter()
        .any(|pa| b.iter().any(|pb| path_pair_overlap(pa, pb)))
}

/// Component-wise containment test on two already-normalized paths.
///
/// `src/a` overlaps `src/a/b` (ancestor) and `src/a` (equal); it does NOT
/// overlap `src/ab` (sibling sharing a string prefix but not a path prefix).
/// A relative vs absolute pair is treated as overlapping (conservative — we
/// cannot prove they resolve to disjoint locations).
fn path_pair_overlap(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let a_abs = a.starts_with('/');
    let b_abs = b.starts_with('/');
    if a_abs != b_abs {
        // Mixed absolute/relative: cannot prove disjoint, assume overlap.
        return true;
    }
    let mut ca = a.split('/').filter(|c| !c.is_empty());
    let mut cb = b.split('/').filter(|c| !c.is_empty());
    loop {
        match (ca.next(), cb.next()) {
            // One path ran out while matching: it is an ancestor of the other.
            (None, _) | (_, None) => return true,
            (Some(x), Some(y)) if x == y => continue,
            // First differing component: disjoint subtrees.
            (Some(_), Some(_)) => return false,
        }
    }
}

/// Lexically normalize a filesystem path for overlap comparison.
///
/// Pure (no filesystem access, no symlink resolution): trims surrounding
/// whitespace, collapses repeated separators, drops `.` components, and
/// resolves interior `..` against preceding components. Returns `None` for an
/// empty/whitespace-only input. Does not canonicalize `~` or relative-vs-cwd —
/// that is intentional; comparison falls back to the conservative
/// absolute/relative mismatch rule above.
pub fn normalize_path(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let absolute = trimmed.starts_with('/');
    let mut stack: Vec<&str> = Vec::new();
    for comp in trimmed.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                // Pop a real component; keep leading `..` for relative paths.
                if matches!(stack.last(), Some(&c) if c != "..") {
                    stack.pop();
                } else if !absolute {
                    stack.push("..");
                }
            }
            other => stack.push(other),
        }
    }
    let joined = stack.join("/");
    Some(if absolute {
        format!("/{joined}")
    } else if joined.is_empty() {
        ".".to_string()
    } else {
        joined
    })
}

/// Whether a whole batch of claims may dispatch in parallel.
///
/// `true` iff no pair of claims conflicts. The harness applies this only after
/// it has already confirmed `parallel_tool_concurrency >= 2` and `len >= 2`,
/// so the empty / single-element cases (vacuously `true`) never reach the
/// parallel path on their own. O(n²) over the batch, but n is the number of
/// tool calls in one turn — small.
pub fn batch_parallelizable(claims: &[ConcurrencyClaim]) -> bool {
    for (i, a) in claims.iter().enumerate() {
        for b in &claims[i + 1..] {
            if claims_conflict(a, b) {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(items: &[&str]) -> ConcurrencyClaim {
        ConcurrencyClaim::paths(items.iter().copied())
    }

    #[test]
    fn normalize_handles_dots_and_separators() {
        assert_eq!(normalize_path("  src//a/./b  ").as_deref(), Some("src/a/b"));
        assert_eq!(normalize_path("/src/a/../b").as_deref(), Some("/src/b"));
        assert_eq!(normalize_path("a/b/../../c").as_deref(), Some("c"));
        assert_eq!(normalize_path("../x").as_deref(), Some("../x"));
        assert_eq!(normalize_path(".").as_deref(), Some("."));
        assert_eq!(normalize_path("   ").as_deref(), None);
        assert_eq!(normalize_path("").as_deref(), None);
    }

    #[test]
    fn shared_calls_never_conflict() {
        assert!(!claims_conflict(
            &ConcurrencyClaim::Shared,
            &ConcurrencyClaim::Shared
        ));
        assert!(batch_parallelizable(&[
            ConcurrencyClaim::Shared,
            ConcurrencyClaim::Shared,
            ConcurrencyClaim::Shared,
        ]));
    }

    #[test]
    fn shared_conflicts_with_any_exclusive() {
        assert!(claims_conflict(
            &ConcurrencyClaim::Shared,
            &paths(&["src/a.rs"])
        ));
        assert!(claims_conflict(
            &paths(&["src/a.rs"]),
            &ConcurrencyClaim::Shared
        ));
        assert!(claims_conflict(
            &ConcurrencyClaim::Shared,
            &ConcurrencyClaim::global()
        ));
    }

    #[test]
    fn global_conflicts_with_everything() {
        assert!(claims_conflict(&ConcurrencyClaim::global(), &paths(&["x"])));
        assert!(claims_conflict(
            &ConcurrencyClaim::global(),
            &ConcurrencyClaim::global()
        ));
    }

    #[test]
    fn disjoint_paths_do_not_conflict() {
        assert!(!claims_conflict(
            &paths(&["src/a.rs"]),
            &paths(&["src/b.rs"])
        ));
        // Sibling string-prefix but distinct path components.
        assert!(!claims_conflict(&paths(&["src/a"]), &paths(&["src/ab"])));
        assert!(batch_parallelizable(&[
            paths(&["a.txt"]),
            paths(&["b.txt"]),
            paths(&["c.txt"]),
        ]));
    }

    #[test]
    fn same_path_conflicts() {
        assert!(claims_conflict(
            &paths(&["src/a.rs"]),
            &paths(&["src/a.rs"])
        ));
        // Different normalizations of the same path still conflict.
        assert!(claims_conflict(
            &paths(&["src/./a.rs"]),
            &paths(&["src/a.rs"])
        ));
        assert!(!batch_parallelizable(&[
            paths(&["src/a.rs"]),
            paths(&["src/a.rs"]),
        ]));
    }

    #[test]
    fn nested_paths_conflict() {
        assert!(claims_conflict(&paths(&["src/a"]), &paths(&["src/a/b.rs"])));
        assert!(claims_conflict(&paths(&["src/a/b.rs"]), &paths(&["src/a"])));
    }

    #[test]
    fn absolute_vs_relative_is_conservative() {
        assert!(claims_conflict(&paths(&["/work/a.rs"]), &paths(&["a.rs"])));
    }

    #[test]
    fn multi_path_scope_conflicts_if_any_overlaps() {
        // move A -> B conflicts with delete B.
        assert!(claims_conflict(
            &paths(&["src/a.rs", "src/b.rs"]),
            &paths(&["src/b.rs"])
        ));
        assert!(!claims_conflict(
            &paths(&["src/a.rs", "src/b.rs"]),
            &paths(&["src/c.rs", "src/d.rs"])
        ));
    }

    #[test]
    fn empty_path_scope_degrades_to_global() {
        let claim = ConcurrencyClaim::paths(Vec::<String>::new());
        assert_eq!(claim, ConcurrencyClaim::global());
    }

    #[test]
    fn mixed_shared_and_exclusive_batch_is_serial() {
        assert!(!batch_parallelizable(&[
            ConcurrencyClaim::Shared,
            paths(&["src/a.rs"]),
        ]));
    }

    #[test]
    fn single_or_empty_is_vacuously_parallel() {
        assert!(batch_parallelizable(&[]));
        assert!(batch_parallelizable(&[ConcurrencyClaim::global()]));
    }
}
