//! Agent-stamped multi-selection for the memory console.

use std::collections::HashSet;

/// A row's full identity: the partition that owns it, plus its id within that
/// partition.
///
/// The partition half is not decoration. A note id is a *path*
/// (`category/filename` over a small, fixed category set), and since the
/// gateway's enumerating readers resolve a base persona id into the union
/// `[org tier, this session's partition]`
/// (`gateway::handlers::memory_scope::read_partitions`), one list can hold
/// `preference/coding-style` from `main` **and** from `main__u-owner` at the
/// same time. They are two different notes. Addressing either one — opening
/// the drawer, exporting the body, deleting it — needs the partition the row
/// itself reported, not the base id the agent picker is showing.
///
/// Raw rows carry a partition too, and it is stored here for the same reason
/// even though `memory.delete` resolves ownership server-side from the bare
/// row id: a selection that held partitions for one layer and not the other
/// would be two shapes wearing one type.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RowRef {
    /// The partition that owns this row — `CompressedFact::agent_id` /
    /// `MemoryEntry::agent_id`, straight off the wire. Never the agent picker's
    /// value.
    pub partition: String,
    /// Note path, or raw-memory id.
    pub id: String,
}

impl RowRef {
    /// Build a reference from a row's own reported partition and id.
    pub fn new(partition: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            partition: partition.into(),
            id: id.into(),
        }
    }
}

/// The rows ticked in the memory list, together with the agent they were ticked
/// under.
///
/// Two different scopes are at work here and conflating them was the original
/// bug. [`RowRef::partition`] answers "which store does this row live in",
/// which is now per-row because one list spans a partition union. The `agent`
/// stamp below answers "which agent's console were these ticked in", which is
/// still one value for the whole selection — switching the agent picker from
/// `main` to `researcher` must not carry ticks across, and the two agents'
/// notes collide on path just as readily as two partitions do.
///
/// Stamping the agent into the selection makes the cross-agent mistake
/// unrepresentable rather than merely unlikely. A selection is mono-agent by
/// construction ([`Self::toggle`] and [`Self::extend`] under a new agent start
/// a fresh set rather than growing the old one), and every reader must name the
/// agent it is acting for ([`Self::ids_for`], [`Self::contains`],
/// [`Self::len_for`]), getting nothing back on a mismatch. Checkboxes therefore
/// empty themselves on a switch and batch actions cannot reach a store other
/// than the one the boxes were ticked in — neither depends on
/// effect-scheduling order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentSelection {
    agent: String,
    ids: HashSet<RowRef>,
}

impl AgentSelection {
    /// Toggle `row` under `agent`. A different agent than the one stamped
    /// starts a fresh selection rather than merging into the old one.
    pub fn toggle(&mut self, agent: &str, row: RowRef) {
        self.rebase(agent);
        if !self.ids.remove(&row) {
            self.ids.insert(row);
        }
    }

    /// Add every row under `agent` (the select-page action).
    pub fn extend(&mut self, agent: &str, rows: impl IntoIterator<Item = RowRef>) {
        self.rebase(agent);
        self.ids.extend(rows);
    }

    /// Remove every row (the deselect-page action).
    ///
    /// Deliberately does not rebase: removal only ever narrows the set, and the
    /// only caller reaches it via [`Self::contains`], which is already false
    /// under a stale stamp — so a stale set can never be *reached* here, and if
    /// it somehow were, narrowing is the safe direction.
    pub fn remove_all(&mut self, rows: &[RowRef]) {
        for row in rows {
            self.ids.remove(row);
        }
    }

    /// Whether the row `(partition, id)` is ticked *for `agent`*.
    #[must_use]
    pub fn contains(&self, agent: &str, partition: &str, id: &str) -> bool {
        self.agent == agent && self.ids.contains(&RowRef::new(partition, id))
    }

    /// How many rows are ticked for `agent`; zero under any other.
    #[must_use]
    pub fn len_for(&self, agent: &str) -> usize {
        if self.agent == agent {
            self.ids.len()
        } else {
            0
        }
    }

    /// The ticked rows, but only if they were ticked under `agent`. Every
    /// mutating consumer goes through this; the mismatch case yields an empty
    /// set, which every caller already treats as "nothing to do".
    #[must_use]
    pub fn ids_for(&self, agent: &str) -> HashSet<RowRef> {
        if self.agent == agent {
            self.ids.clone()
        } else {
            HashSet::new()
        }
    }

    /// Drop everything, keeping the stamp irrelevant (an empty selection reads
    /// as empty for every agent).
    pub fn clear(&mut self) {
        self.ids.clear();
    }

    fn rebase(&mut self, agent: &str) {
        if self.agent != agent {
            self.agent = agent.to_string();
            self.ids.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection_under(agent: &str, partition: &str, ids: &[&str]) -> AgentSelection {
        let mut s = AgentSelection::default();
        for id in ids {
            s.toggle(agent, RowRef::new(partition, *id));
        }
        s
    }

    #[test]
    fn ticking_and_unticking_round_trips_under_one_agent() {
        let mut s = selection_under("alice", "main", &["preference/coding-style"]);
        assert!(s.contains("alice", "main", "preference/coding-style"));
        assert_eq!(s.len_for("alice"), 1);
        s.toggle("alice", RowRef::new("main", "preference/coding-style"));
        assert!(!s.contains("alice", "main", "preference/coding-style"));
        assert_eq!(s.len_for("alice"), 0);
    }

    #[test]
    fn switching_agents_empties_the_selection_rather_than_merging() {
        let mut s = selection_under("alice", "main", &["preference/coding-style"]);
        s.toggle("bob", RowRef::new("main", "preference/other"));
        assert_eq!(s.len_for("alice"), 0);
        assert_eq!(s.len_for("bob"), 1);
        assert!(!s.contains("alice", "main", "preference/coding-style"));
    }

    #[test]
    fn reading_under_the_wrong_agent_yields_nothing() {
        let s = selection_under("alice", "main", &["a", "b"]);
        assert!(s.ids_for("bob").is_empty());
        assert_eq!(s.len_for("bob"), 0);
        assert!(!s.contains("bob", "main", "a"));
    }

    #[test]
    fn extend_under_a_new_agent_replaces_rather_than_grows() {
        let mut s = selection_under("alice", "main", &["a"]);
        s.extend("bob", [RowRef::new("main", "b"), RowRef::new("main", "c")]);
        assert_eq!(s.len_for("bob"), 2);
        assert_eq!(s.len_for("alice"), 0);
    }

    #[test]
    fn clearing_reads_as_empty_for_every_agent() {
        let mut s = selection_under("alice", "main", &["a"]);
        s.clear();
        assert_eq!(s.len_for("alice"), 0);
        assert_eq!(s.len_for("bob"), 0);
    }

    /// The property the partition half exists for: the same note path in two
    /// partitions is two rows, tickable and untickable independently.
    ///
    /// Without it, one checkbox drove both — and the batch verbs, which address
    /// by (partition, path), would have deleted whichever one the picker's base
    /// id happened to name.
    #[test]
    fn the_same_path_in_two_partitions_is_two_independent_rows() {
        let mut s = AgentSelection::default();
        s.toggle("main", RowRef::new("main", "preference/coding-style"));
        s.toggle(
            "main",
            RowRef::new("main__u-owner", "preference/coding-style"),
        );

        assert_eq!(s.len_for("main"), 2, "two partitions, two rows");
        assert!(s.contains("main", "main", "preference/coding-style"));
        assert!(s.contains("main", "main__u-owner", "preference/coding-style"));

        // Unticking one must leave the other alone.
        s.toggle("main", RowRef::new("main", "preference/coding-style"));
        assert!(!s.contains("main", "main", "preference/coding-style"));
        assert!(
            s.contains("main", "main__u-owner", "preference/coding-style"),
            "unticking the org-tier row must not untick the personal one"
        );
    }

    /// A ticked row is not reachable under a partition it does not belong to,
    /// which is what stops a batch verb from being aimed at the wrong store.
    #[test]
    fn a_row_is_not_ticked_under_a_partition_it_does_not_belong_to() {
        let s = selection_under("main", "main__u-owner", &["wiki/note"]);
        assert!(s.contains("main", "main__u-owner", "wiki/note"));
        assert!(
            !s.contains("main", "main", "wiki/note"),
            "the org tier holds no such ticked row"
        );
    }
}
