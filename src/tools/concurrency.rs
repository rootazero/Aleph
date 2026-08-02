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
    /// Touches exactly this set of cluster nodes (remote execution arms),
    /// keyed by the `node` selector the caller named. Two `Nodes` scopes
    /// conflict only when they name a common node, so `node_invoke` calls
    /// against *different* machines run concurrently instead of serializing
    /// the whole batch behind a `Global` claim.
    ///
    /// A node's filesystem is a different machine from the center's, so a
    /// `Nodes` scope never conflicts with a `Paths` scope. (`node_file` is
    /// deliberately NOT modelled here: it straddles both — it reads/writes a
    /// center-local path *and* the node — so it keeps the conservative
    /// [`ExclusiveScope::Global`] claim.)
    Nodes(BTreeSet<String>),
    /// Touches exactly this set of *local* agent sessions (delegation arms,
    /// keyed by the resolved target session key). Two `Sessions` scopes
    /// conflict only when they name a common session, so fan-out delegations
    /// (`session_send` blocks up to its `timeout_seconds` waiting on the
    /// child) to *different* sessions run concurrently instead of paying
    /// N × wait serially.
    ///
    /// Unlike [`ExclusiveScope::Nodes`], a delegated session's run executes
    /// on THIS machine and may touch arbitrary center resources (files,
    /// stores) while the parent batch is still in flight — so a `Sessions`
    /// scope conservatively conflicts with every *other* scope kind
    /// (`Global`, `Paths`, `Nodes`): only disjoint sibling delegations
    /// parallelize. Do not "optimize" the cross-kind arms to no-conflict —
    /// the child's footprint is unknowable here.
    Sessions(BTreeSet<String>),
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
            Self::Exclusive {
                scope: ExclusiveScope::Global,
            }
        } else {
            Self::Exclusive {
                scope: ExclusiveScope::Paths(set),
            }
        }
    }

    /// Convenience constructor for a bounded cluster-node scope. Node keys are
    /// taken verbatim (they are the caller's `node` selector — a name or id).
    /// An empty set degrades to [`ExclusiveScope::Global`]: we could not pin
    /// which machines the call reaches, so assume all of them.
    pub fn nodes<I, S>(nodes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let set: BTreeSet<String> = nodes
            .into_iter()
            .map(|n| n.as_ref().trim().to_string())
            .filter(|n| !n.is_empty())
            .collect();
        if set.is_empty() {
            Self::global()
        } else {
            Self::Exclusive {
                scope: ExclusiveScope::Nodes(set),
            }
        }
    }

    /// Convenience constructor for a bounded local-session scope. Session
    /// keys are taken verbatim (the caller's resolved target session key).
    /// An empty set degrades to [`ExclusiveScope::Global`]: we could not pin
    /// which session the call reaches, so assume the worst.
    pub fn sessions<I, S>(sessions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let set: BTreeSet<String> = sessions
            .into_iter()
            .map(|s| s.as_ref().trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if set.is_empty() {
            Self::global()
        } else {
            Self::Exclusive {
                scope: ExclusiveScope::Sessions(set),
            }
        }
    }

    /// The whole-world exclusive claim. Conflicts with everything.
    #[must_use]
    pub const fn global() -> Self {
        Self::Exclusive {
            scope: ExclusiveScope::Global,
        }
    }
}

/// Whether two claims conflict — i.e. they must NOT run concurrently.
///
/// Truth table:
/// * `Shared`      vs `Shared`           → no conflict (reads parallelize)
/// * `Shared`      vs `Exclusive(_)`     → conflict (read may observe a torn write)
/// * `Global`      vs anything           → conflict (unbounded footprint)
/// * `Paths(a)`    vs `Paths(b)`         → conflict iff the path sets overlap
/// * `Nodes(a)`    vs `Nodes(b)`         → conflict iff they name a common node
/// * `Paths(_)`    vs `Nodes(_)`         → no conflict (different machines)
/// * `Sessions(a)` vs `Sessions(b)`      → conflict iff they name a common session
/// * `Sessions(_)` vs any other scope    → conflict (the delegated run's local
///   footprint is unknowable — see [`ExclusiveScope::Sessions`])
#[must_use]
pub fn claims_conflict(a: &ConcurrencyClaim, b: &ConcurrencyClaim) -> bool {
    use ConcurrencyClaim::{Exclusive, Shared};
    match (a, b) {
        (Shared, Shared) => false,
        (Shared, Exclusive { .. }) | (Exclusive { .. }, Shared) => true,
        (Exclusive { scope: sa }, Exclusive { scope: sb }) => scopes_conflict(sa, sb),
    }
}

fn scopes_conflict(a: &ExclusiveScope, b: &ExclusiveScope) -> bool {
    use ExclusiveScope::{Global, Nodes, Paths, Sessions};
    match (a, b) {
        (Global, _) | (_, Global) => true,
        (Paths(pa), Paths(pb)) => paths_overlap(pa, pb),
        (Nodes(na), Nodes(nb)) => !na.is_disjoint(nb),
        (Sessions(sa), Sessions(sb)) => !sa.is_disjoint(sb),
        // A delegated session runs on THIS machine and can touch arbitrary
        // center resources while the batch is in flight, so a bounded
        // `Sessions` scope conservatively conflicts with every other kind.
        (Sessions(_), _) | (_, Sessions(_)) => true,
        // A remote node's state and the center's filesystem are on different
        // machines — a bounded claim on one cannot touch the other.
        (Paths(_), Nodes(_)) | (Nodes(_), Paths(_)) => false,
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
///
/// `\` counts as a separator on every platform, not just Windows. Model-written
/// paths arrive as raw argument strings, so on Windows the same file reaches
/// this function spelled both ways; splitting on `/` alone left `C:\work\a.rs`
/// as one opaque component that differed from `C:/work/a.rs` at the first
/// component, and the two writes parallelized onto the same file. Folding
/// unconditionally is the safe direction on Unix too: a filename containing a
/// literal backslash is legal but vanishingly rare there, and the only cost of
/// splitting it is a spurious serialization — the same trade the read-only
/// allowlist makes (a miss loses parallelism, never correctness).
#[must_use]
pub fn normalize_path(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let absolute = trimmed.starts_with(['/', '\\']);
    let mut stack: Vec<&str> = Vec::new();
    for comp in trimmed.split(['/', '\\']) {
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
#[must_use]
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

/// Partition a batch of claims into contiguous, order-preserving groups such
/// that every group is internally parallelizable while the relative order of
/// any two *conflicting* claims is preserved across group boundaries.
///
/// The harness dispatches the returned groups sequentially and runs the calls
/// *within* each group concurrently, which makes the schedule sound:
/// * **intra-group safety** — no pair inside a group conflicts, so concurrent
///   execution observes no torn side effects; and
/// * **inter-group ordering** — group `k` fully completes before group `k+1`
///   starts, so a conflict that spans a boundary is serialized.
///
/// This strictly generalizes the whole-batch [`batch_parallelizable`] gate: a
/// fully-parallel batch yields a single group, a fully-serial batch yields `n`
/// singletons, and a mixed batch (e.g. three reads then a `bash`) yields a
/// parallel read group followed by a serial `bash` group. None of the reference
/// agents do this — hermes-agent's `_should_parallelize_tool_batch`, openclaw's
/// and codex's per-batch gates all make a single whole-batch parallel-or-serial
/// decision, forcing the reads onto the serial path the moment one `bash` joins
/// the batch.
///
/// Greedy and order-preserving: walk left to right, extending the current group
/// while the next claim conflicts with no claim already in it; open a fresh
/// group at the first conflict. Because a new group only ever begins at the
/// current index, groups are contiguous half-open `[start, end)` ranges that
/// tile `0..claims.len()`. O(n²) worst case over a batch that is, in practice,
/// a handful of tool calls.
///
/// Returns an empty vec for an empty batch; otherwise the ranges always cover
/// `0..claims.len()` with no gaps or overlaps.
#[must_use]
pub fn partition_parallel_groups(claims: &[ConcurrencyClaim]) -> Vec<(usize, usize)> {
    let mut groups = Vec::new();
    let mut start = 0usize;
    for i in 1..claims.len() {
        // Open a new group when claim[i] conflicts with any member already in
        // the current group. Members are pairwise non-conflicting by induction,
        // so the group stays internally parallelizable.
        let conflicts = claims[start..i]
            .iter()
            .any(|c| claims_conflict(c, &claims[i]));
        if conflicts {
            groups.push((start, i));
            start = i;
        }
    }
    if !claims.is_empty() {
        groups.push((start, claims.len()));
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(items: &[&str]) -> ConcurrencyClaim {
        ConcurrencyClaim::paths(items.iter().copied())
    }

    fn nodes(items: &[&str]) -> ConcurrencyClaim {
        ConcurrencyClaim::nodes(items.iter().copied())
    }

    #[test]
    fn distinct_nodes_parallelize_but_the_same_node_serializes() {
        assert!(
            !claims_conflict(&nodes(&["worker-1"]), &nodes(&["worker-2"])),
            "invokes on different machines must run concurrently"
        );
        assert!(
            claims_conflict(&nodes(&["worker-1"]), &nodes(&["worker-1"])),
            "two invokes into one node share its bash session workspace"
        );
        // Overlapping sets conflict on the common member.
        assert!(claims_conflict(
            &nodes(&["worker-1", "worker-2"]),
            &nodes(&["worker-2"])
        ));
    }

    #[test]
    fn node_scope_is_disjoint_from_center_paths_but_not_from_global() {
        assert!(
            !claims_conflict(&nodes(&["worker-1"]), &paths(&["src/main.rs"])),
            "a remote node and the center's filesystem are different machines"
        );
        assert!(claims_conflict(
            &nodes(&["worker-1"]),
            &ConcurrencyClaim::global()
        ));
        assert!(
            claims_conflict(&nodes(&["worker-1"]), &ConcurrencyClaim::Shared),
            "a read may still observe center-side effects of a remote call"
        );
    }

    #[test]
    fn empty_node_selector_degrades_to_global() {
        assert_eq!(
            ConcurrencyClaim::nodes(Vec::<&str>::new()),
            ConcurrencyClaim::global()
        );
        assert_eq!(ConcurrencyClaim::nodes(["   "]), ConcurrencyClaim::global());
    }

    fn sessions(items: &[&str]) -> ConcurrencyClaim {
        ConcurrencyClaim::sessions(items.iter().copied())
    }

    #[test]
    fn distinct_sessions_parallelize_but_the_same_session_serializes() {
        assert!(
            !claims_conflict(&sessions(&["agent:a:main"]), &sessions(&["agent:b:main"])),
            "fan-out delegations to different sessions must run concurrently"
        );
        assert!(
            claims_conflict(&sessions(&["agent:a:main"]), &sessions(&["agent:a:main"])),
            "two sends into one session must keep their relative order"
        );
    }

    #[test]
    fn session_scope_conservatively_conflicts_with_every_other_scope_kind() {
        // The delegated run executes locally and its footprint is unknowable,
        // so Sessions only ever parallelizes with disjoint sibling Sessions.
        assert!(claims_conflict(
            &sessions(&["agent:a:main"]),
            &paths(&["src/a.rs"])
        ));
        assert!(claims_conflict(
            &sessions(&["agent:a:main"]),
            &nodes(&["worker-1"])
        ));
        assert!(claims_conflict(
            &sessions(&["agent:a:main"]),
            &ConcurrencyClaim::global()
        ));
        assert!(claims_conflict(
            &sessions(&["agent:a:main"]),
            &ConcurrencyClaim::Shared
        ));
    }

    #[test]
    fn empty_session_selector_degrades_to_global() {
        assert_eq!(
            ConcurrencyClaim::sessions(Vec::<&str>::new()),
            ConcurrencyClaim::global()
        );
        assert_eq!(
            ConcurrencyClaim::sessions(["  "]),
            ConcurrencyClaim::global()
        );
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
    fn windows_separators_fold_onto_the_same_scope() {
        // Two spellings of one file must not be judged disjoint. Splitting on
        // `/` alone made `C:\work\a.rs` a single opaque component, so it
        // differed from `C:/work/a.rs` at the first component and the two
        // claims parallelized — concurrent writes to the same file, which is
        // the exact race this module exists to prevent.
        assert_eq!(
            normalize_path(r"C:\work\a.rs"),
            normalize_path("C:/work/a.rs")
        );
        assert!(claims_conflict(
            &ConcurrencyClaim::paths([r"C:\work\a.rs"]),
            &ConcurrencyClaim::paths(["C:/work/a.rs"]),
        ));
        // Ancestor containment must survive the fold too.
        assert!(claims_conflict(
            &ConcurrencyClaim::paths([r"C:\work"]),
            &ConcurrencyClaim::paths([r"C:\work\a.rs"]),
        ));
        // Siblings still parallelize.
        assert!(!claims_conflict(
            &ConcurrencyClaim::paths([r"C:\work\a.rs"]),
            &ConcurrencyClaim::paths([r"C:\work\b.rs"]),
        ));
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

    // ---- partition_parallel_groups ----------------------------------------

    /// Every partition must tile `0..n` with contiguous, non-overlapping ranges
    /// and every group must itself be parallelizable.
    fn assert_valid_partition(claims: &[ConcurrencyClaim], groups: &[(usize, usize)]) {
        if claims.is_empty() {
            assert!(groups.is_empty());
            return;
        }
        assert_eq!(groups.first().unwrap().0, 0, "must start at 0");
        assert_eq!(groups.last().unwrap().1, claims.len(), "must end at n");
        for w in groups.windows(2) {
            assert_eq!(w[0].1, w[1].0, "groups must be contiguous (no gap/overlap)");
        }
        for &(s, e) in groups {
            assert!(s < e, "groups are non-empty");
            assert!(
                batch_parallelizable(&claims[s..e]),
                "group [{s},{e}) must be internally parallelizable",
            );
        }
    }

    #[test]
    fn partition_empty_is_empty() {
        assert!(partition_parallel_groups(&[]).is_empty());
    }

    #[test]
    fn partition_all_shared_is_one_group() {
        let claims = vec![
            ConcurrencyClaim::Shared,
            ConcurrencyClaim::Shared,
            ConcurrencyClaim::Shared,
        ];
        let groups = partition_parallel_groups(&claims);
        assert_eq!(groups, vec![(0, 3)]);
        assert_valid_partition(&claims, &groups);
    }

    #[test]
    fn partition_disjoint_writes_is_one_group() {
        let claims = vec![paths(&["a.txt"]), paths(&["b.txt"]), paths(&["c.txt"])];
        let groups = partition_parallel_groups(&claims);
        assert_eq!(groups, vec![(0, 3)]);
        assert_valid_partition(&claims, &groups);
    }

    #[test]
    fn partition_all_global_is_n_singletons() {
        let claims = vec![
            ConcurrencyClaim::global(),
            ConcurrencyClaim::global(),
            ConcurrencyClaim::global(),
        ];
        let groups = partition_parallel_groups(&claims);
        assert_eq!(groups, vec![(0, 1), (1, 2), (2, 3)]);
        assert_valid_partition(&claims, &groups);
    }

    #[test]
    fn partition_reads_then_bash_splits_at_boundary() {
        // The canonical win: N reads + one whole-world bash. The reads
        // parallelize; the bash serializes after them. A whole-batch decider
        // would run all four serially.
        let claims = vec![
            ConcurrencyClaim::Shared,
            ConcurrencyClaim::Shared,
            ConcurrencyClaim::Shared,
            ConcurrencyClaim::global(),
        ];
        let groups = partition_parallel_groups(&claims);
        assert_eq!(groups, vec![(0, 3), (3, 4)]);
        assert_valid_partition(&claims, &groups);
    }

    #[test]
    fn partition_bash_between_reads_isolates_bash() {
        // reads | bash | reads — the bash conflicts with reads on both sides,
        // so it sits alone between two parallel read groups.
        let claims = vec![
            ConcurrencyClaim::Shared,
            ConcurrencyClaim::Shared,
            ConcurrencyClaim::global(),
            ConcurrencyClaim::Shared,
            ConcurrencyClaim::Shared,
        ];
        let groups = partition_parallel_groups(&claims);
        assert_eq!(groups, vec![(0, 2), (2, 3), (3, 5)]);
        assert_valid_partition(&claims, &groups);
    }

    #[test]
    fn partition_shared_then_disjoint_write_serializes() {
        // Shared conflicts with ANY exclusive (its read footprint is unknown),
        // so a read followed by a disjoint write must serialize — the partition
        // never merges them, preserving soundness over the false "disjoint
        // paths" intuition.
        let claims = vec![ConcurrencyClaim::Shared, paths(&["d.rs"])];
        let groups = partition_parallel_groups(&claims);
        assert_eq!(groups, vec![(0, 1), (1, 2)]);
        assert_valid_partition(&claims, &groups);
    }

    #[test]
    fn partition_same_path_writes_serialize() {
        let claims = vec![paths(&["src/a.rs"]), paths(&["src/a.rs"])];
        let groups = partition_parallel_groups(&claims);
        assert_eq!(groups, vec![(0, 1), (1, 2)]);
        assert_valid_partition(&claims, &groups);
    }

    #[test]
    fn partition_matches_whole_batch_gate_when_fully_parallel() {
        // When the whole batch is parallelizable, the partition collapses to a
        // single group — i.e. the new path never *loses* parallelism the old
        // gate would have granted.
        let claims = vec![paths(&["a"]), paths(&["b"]), ConcurrencyClaim::Shared];
        // Shared conflicts with the exclusive path claims, so NOT fully parallel.
        assert!(!batch_parallelizable(&claims));
        let groups = partition_parallel_groups(&claims);
        assert_valid_partition(&claims, &groups);

        let parallel = vec![paths(&["a"]), paths(&["b"]), paths(&["c"])];
        assert!(batch_parallelizable(&parallel));
        assert_eq!(partition_parallel_groups(&parallel), vec![(0, 3)]);
    }
}
