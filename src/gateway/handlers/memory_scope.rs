//! The partition contract for the gateway's memory RPC face.
//!
//! Every memory WRITER composes the current session's scope
//! (`project_scope::session_write_id`, converged for the tool face on
//! `BuiltinToolRegistry::caller_memory_partition`), and a zero-config loopback
//! Panel session already resolves to `Personal(u-owner)` — the RPC dispatcher
//! installs it for every request (`server::handler::dispatch_with_caller_context`).
//! So notes, raws, corrections and dream state written from a stock
//! single-machine install land in `main__u-owner`.
//!
//! The RPC READERS took the base id the agent picker holds (`main`) and queried
//! that partition exactly. On a machine where `note_manage` had just created
//! 1040 notes, `memory.listFacts(agent_id="main").total` answered **0**: the
//! Vault's note list, its facets, the stat cards and the galaxy were all
//! structurally empty on a stock install, with no error anywhere. The tool face
//! was swept for this twice (FEATURE_LOCATOR §5.22 round-3 ②, §5.22 ⑪) and the
//! gateway face was in neither sweep — the criterion being **一个动词有 N 个面
//! 时，「谁能看」要在每个面用同一个推导**.
//!
//! ## The two questions, and why one field answers both
//!
//! A caller-supplied `agent_id` is one of exactly two things, and which one is
//! decidable from the value alone (`project_scope::is_composed_id`) rather than
//! from which method it arrived on:
//!
//! - a **base persona id** (`main`) — what the agent picker holds. It names an
//!   agent, not a partition, and the honest reading is "this session's view of
//!   that agent": [`read_partitions`] resolves it to
//!   `project_scope::session_read_ids`, the union `[org tier, this session's
//!   partition]` that `memory_search` and the prompt assembler already use.
//! - an **already-composed partition id** (`main__u-alice`) — an explicit
//!   address, which only an enumerating response could have handed the caller.
//!   Honored verbatim, exactly as before this module existed, which is what
//!   keeps the P1 isolation tests meaningful: they assert that Bob naming
//!   Alice's partition is refused, not silently rewritten to his own.
//!
//! Composing a composed id is the failure this split exists to make
//! unreachable: `session_read_ids("main__u-owner")` under an active personal
//! scope yields `main__u-owner__u-owner`, a partition nobody writes.
//!
//! ## Order of operations
//!
//! Compose **after** the visibility gate, never before. `partition_visible`
//! judges the id the caller actually sent; handing it a string the caller never
//! wrote would gate a decision the caller did not make. Every handler here
//! therefore reads: parse → `partition_visible(base)` → [`read_partitions`].
//!
//! ## Why `project_scoped: false`
//!
//! `session_read_ids`' `project_scoped`/`project_root` arguments are consulted
//! **only** when there is no session scope at all. On this face there either is
//! one (and the arguments are ignored) or there is not — an internal, cron or
//! A2A call with no `caller_user` — in which case `false`/`None` collapses the
//! result to `[base]`, byte-for-byte what every one of these handlers did
//! before. The legacy project-directory feature was never reachable from the
//! gateway face and this does not introduce it; adding it would need a config
//! handle none of these handlers hold.

/// The partitions an **enumerating** memory RPC must read for a
/// caller-supplied `agent_id`.
///
/// Enumerating = "show me what is in here": list, count, search, stats,
/// retrieval. Contrast the **addressing** verbs (`graph.node_detail`,
/// `graph.update_note`, `graph.rename_note`, `graph.delete_note`,
/// `memory.delete`), which act on one row the caller already holds an address
/// for; those take the partition verbatim — the value an enumerating response
/// put on that row — and must NOT come through here, because widening a delete
/// to a union is how you delete the wrong person's note.
///
/// Never empty: the base id is always a member, so a caller that resolves
/// through this can always be handed to a single-partition backend method by
/// taking the first element if a multi-partition one does not exist yet.
#[must_use]
pub fn read_partitions(agent_id: &str) -> Vec<String> {
    if crate::memory::project_scope::is_composed_id(agent_id) {
        // An explicit partition address. See the module doc: composing this
        // would produce a ghost partition, and rewriting it to the caller's
        // own would turn a refusal into a silent substitution.
        return vec![agent_id.to_string()];
    }
    crate::memory::project_scope::session_read_ids(agent_id, false, None)
}

/// The single partition an enumerating RPC must read when the surface it feeds
/// cannot represent more than one — today only the galaxy graph, whose
/// `NoteNodeDto` carries no `agent_id`, so two partitions holding the same
/// `category/filename` would collide into one node with no way to tell them
/// apart.
///
/// This is the session's OWN partition (`session_write_id`), not the org tier:
/// of the two members of [`read_partitions`] it is the one that actually holds
/// what this session wrote, which is what a user looking at the galaxy is
/// asking about. The org tier being invisible there is a **known, narrower**
/// gap than the one this module closes — recorded in FEATURE_LOCATOR §6.7 —
/// not an oversight.
#[must_use]
pub fn primary_read_partition(agent_id: &str) -> String {
    if crate::memory::project_scope::is_composed_id(agent_id) {
        return agent_id.to_string();
    }
    crate::memory::project_scope::session_write_id(agent_id, false, None)
}

/// Every gateway RPC that ENUMERATES memory, and the file its handler lives in.
///
/// A literal list, and it must stay a short one — the census below is what
/// makes it honest, not the list itself. Membership criterion, stated once:
///
/// > this method answers "show me what is in here" for a caller-supplied
/// > `agent_id` that names an AGENT (a base persona) rather than a partition,
/// > and there is no other control on the surface that reaches the composed
/// > partition.
///
/// Two deliberate non-members, each with the reason recorded on the handler:
/// `dreaming.list_insights` (its `namespaces` index makes every corpus
/// addressable, so its `agent_id` names a corpus) and every ADDRESSING verb —
/// `graph.node_detail`, `graph.update_note`, `graph.rename_note`,
/// `graph.delete_note`, `memory.delete` — which act on one row whose partition
/// the caller already holds, and where widening to a union would act on
/// somebody else's row.
#[cfg(test)]
const ENUMERATING_MEMORY_READERS: &[(&str, &str)] = &[
    ("memory.search", "src/gateway/handlers/memory.rs"),
    ("memory.listFacts", "src/gateway/handlers/memory.rs"),
    ("memory.stats", "src/gateway/handlers/memory.rs"),
    ("memory.list_corrections", "src/gateway/handlers/memory.rs"),
    ("graph.search", "src/gateway/handlers/graph/search.rs"),
    ("graph.query", "src/gateway/handlers/graph/query.rs"),
    ("insights.tools", "src/gateway/handlers/insights.rs"),
    (
        "memory.retrieve_with_trace",
        "src/gateway/handlers/memory_config.rs",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::{with_scope, ScopeAttribution, ScopeId};

    /// The defect this module exists for, stated as an assertion: under the
    /// scope a stock loopback Panel session runs in, an enumerating read of the
    /// base persona must include the partition the writers composed.
    #[tokio::test]
    async fn a_scoped_session_reads_the_partition_its_own_writes_land_in() {
        let write_target = with_scope(Some(ScopeAttribution::personal("u-owner")), async {
            crate::memory::project_scope::session_write_id("main", false, None)
        })
        .await;
        assert_eq!(write_target, "main__u-owner");

        let reads = with_scope(Some(ScopeAttribution::personal("u-owner")), async {
            read_partitions("main")
        })
        .await;
        assert!(
            reads.contains(&write_target),
            "the reader must look where the writer wrote; got {reads:?}"
        );
        assert!(
            reads.contains(&"main".to_string()),
            "the org tier stays readable — pre-P1 notes and every unscoped \
             principal's writes live there; got {reads:?}"
        );
    }

    /// With no session scope (internal / cron / A2A) this is byte-for-byte the
    /// pre-existing behaviour: exactly the bare id, nothing unioned in.
    #[tokio::test]
    async fn an_unscoped_caller_is_unchanged() {
        assert_eq!(read_partitions("main"), vec!["main".to_string()]);
        assert_eq!(primary_read_partition("main"), "main");
    }

    /// An explicit partition address is honored verbatim rather than composed
    /// — otherwise it becomes `main__u-alice__u-alice`, which nothing writes.
    #[tokio::test]
    async fn an_explicit_partition_address_is_never_recomposed() {
        let (reads, primary) = with_scope(Some(ScopeAttribution::personal("u-owner")), async {
            (
                read_partitions("main__u-alice"),
                primary_read_partition("main__u-alice"),
            )
        })
        .await;
        assert_eq!(reads, vec!["main__u-alice".to_string()]);
        assert_eq!(primary, "main__u-alice");
    }

    /// A room shares one partition, and the union is `[org, room]` — never
    /// anybody's personal corpus, not even the creator's. Same shape contract
    /// `project_scope::session_read_ids` asserts one level down; restated here
    /// because this is the function the gateway actually calls.
    #[tokio::test]
    async fn a_room_reads_the_room_and_the_org_tier_and_nothing_else() {
        let reads = with_scope(
            Some(ScopeAttribution {
                owner_user_id: "u-alice".to_string(),
                scope: ScopeId::Project("p-room".to_string()),
            }),
            async { read_partitions("main") },
        )
        .await;
        assert_eq!(
            reads,
            vec!["main".to_string(), "main__p-room".to_string()],
            "asserted on the whole vec, not on `contains`: a third member here \
             would be a cross-user leak with no error anywhere"
        );
    }

    /// Source-level, because the failure is silent in both directions and no
    /// runtime test that constructs a store with a base id and asserts against
    /// that same base id can cross this seam — which is exactly how the tool
    /// face got swept twice while this face was never swept at all.
    ///
    /// The rule is "the file resolves through this module", not "line N does":
    /// a handler is free to take the union or the single-partition arm
    /// depending on what its surface can represent (the galaxy takes
    /// `primary_read_partition`), and pinning the spelling per method would
    /// just be a second, weaker copy of that judgement.
    #[test]
    fn every_enumerating_memory_reader_resolves_the_session_partition() {
        let mut offenders = Vec::new();
        let mut checked = 0usize;

        for (method, file) in ENUMERATING_MEMORY_READERS {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(file);
            let Ok(src) = std::fs::read_to_string(&path) else {
                offenders.push(format!("{method}: cannot read {file}"));
                continue;
            };
            // Comments are documentation, not code: this module is named in
            // several explanatory comments (including the one on the
            // deliberate non-member right next to a member), and a scanner
            // that counts those would pass a file that had stopped calling it.
            let code: String = src
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            checked += 1;
            if !code.contains("memory_scope::read_partitions")
                && !code.contains("memory_scope::primary_read_partition")
            {
                offenders.push(format!(
                    "{method} ({file}): reads the bare persona the agent picker holds, while \
                     every writer composes the session scope"
                ));
            }
        }

        assert_eq!(
            checked,
            ENUMERATING_MEMORY_READERS.len(),
            "the census did not reach every listed file — an unreadable file must \
             not be able to pass as a compliant one"
        );
        assert!(
            offenders.is_empty(),
            "these enumerating memory RPCs read a partition nothing writes to. On a stock \
             loopback install every writer composes `Personal(u-owner)`, so the bare `main` \
             they query is empty and they report that emptiness as fact. Resolve through \
             `gateway::handlers::memory_scope::read_partitions` (or \
             `primary_read_partition` for a surface that cannot represent a union):\n  {}",
            offenders.join("\n  ")
        );
    }

    /// The census above proves each file calls this module; this proves the
    /// list is not quietly shrinking. Both halves are needed: deleting a row
    /// from `ENUMERATING_MEMORY_READERS` would silence the first assertion
    /// without fixing anything.
    #[test]
    fn the_census_covers_every_handler_file_that_reads_memory() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        for (method, file) in ENUMERATING_MEMORY_READERS {
            assert!(
                root.join(file).is_file(),
                "{method} points at {file}, which does not exist — a moved handler \
                 must move this row with it, not drop off the census"
            );
        }
        assert!(
            ENUMERATING_MEMORY_READERS.len() >= 8,
            "this census listed 8 readers when it was written and may only grow; \
             removing one means either the method is gone (delete the row AND say so \
             in FEATURE_LOCATOR §6.7) or somebody narrowed the guard"
        );
    }
}
