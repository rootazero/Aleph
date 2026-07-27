//! Agent-stamped multi-selection for the memory console.

use std::collections::HashSet;

/// The ids ticked in the memory list, together with the agent they were ticked
/// under.
///
/// Note ids are *paths* — `category/filename` over a small, fixed category set
/// — so the same `preference/coding-style` existing under two agents is
/// ordinary, not exotic. A bare `HashSet<String>` cannot answer "whose notes
/// are these?", which forced every batch action to re-read the live agent
/// signal at click time: tick boxes under agent A, switch to B, hit delete, and
/// B's colliding notes are what actually got deleted. Correctness rested
/// entirely on a clear-on-switch effect running before the click.
///
/// Stamping the agent into the selection makes that unrepresentable instead of
/// merely unlikely. A selection is mono-agent by construction ([`Self::toggle`]
/// and [`Self::extend`] under a new agent start a fresh set rather than growing
/// the old one), and every reader must name the agent it is acting for
/// ([`Self::ids_for`], [`Self::contains`], [`Self::len_for`]), getting nothing
/// back on a mismatch. Checkboxes therefore empty themselves on a switch and
/// batch actions cannot reach a store other than the one the boxes were ticked
/// in — neither depends on effect-scheduling order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentSelection {
    agent: String,
    ids: HashSet<String>,
}

impl AgentSelection {
    /// Toggle `id` under `agent`. A different agent than the one stamped
    /// starts a fresh selection rather than merging into the old one.
    pub fn toggle(&mut self, agent: &str, id: &str) {
        self.rebase(agent);
        if !self.ids.remove(id) {
            self.ids.insert(id.to_string());
        }
    }

    /// Add every id under `agent` (the select-page action).
    pub fn extend(&mut self, agent: &str, ids: impl IntoIterator<Item = String>) {
        self.rebase(agent);
        self.ids.extend(ids);
    }

    /// Remove every id (the deselect-page action).
    ///
    /// Deliberately does not rebase: removal only ever narrows the set, and the
    /// only caller reaches it via [`Self::contains`], which is already false
    /// under a stale stamp — so a stale set can never be *reached* here, and if
    /// it somehow were, narrowing is the safe direction.
    pub fn remove_all(&mut self, ids: &[String]) {
        for id in ids {
            self.ids.remove(id);
        }
    }

    /// Whether `id` is ticked *for `agent`*.
    #[must_use]
    pub fn contains(&self, agent: &str, id: &str) -> bool {
        self.agent == agent && self.ids.contains(id)
    }

    /// How many ids are ticked for `agent`; zero under any other.
    #[must_use]
    pub fn len_for(&self, agent: &str) -> usize {
        if self.agent == agent {
            self.ids.len()
        } else {
            0
        }
    }

    /// The ticked ids, but only if they were ticked under `agent`. Every
    /// mutating consumer goes through this; the mismatch case yields an empty
    /// set, which every caller already treats as "nothing to do".
    #[must_use]
    pub fn ids_for(&self, agent: &str) -> HashSet<String> {
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

    fn selection_under(agent: &str, ids: &[&str]) -> AgentSelection {
        let mut s = AgentSelection::default();
        for id in ids {
            s.toggle(agent, id);
        }
        s
    }

    #[test]
    fn ticking_and_unticking_round_trips_under_one_agent() {
        let mut s = selection_under("alice", &["preference/coding-style"]);
        assert!(s.contains("alice", "preference/coding-style"));
        assert_eq!(s.len_for("alice"), 1);
        s.toggle("alice", "preference/coding-style");
        assert!(!s.contains("alice", "preference/coding-style"));
        assert_eq!(s.len_for("alice"), 0);
    }

    #[test]
    fn a_selection_is_invisible_to_another_agent() {
        // The C1 failure mode: a colliding path under a second agent. Without
        // the stamp this read as "selected" and the batch delete sent it to
        // whichever agent happened to be live.
        let s = selection_under("alice", &["preference/coding-style"]);
        assert!(!s.contains("bob", "preference/coding-style"));
        assert_eq!(s.len_for("bob"), 0);
        assert!(s.ids_for("bob").is_empty());
    }

    #[test]
    fn ids_for_yields_the_set_only_to_its_own_agent() {
        let s = selection_under("alice", &["a/one", "a/two"]);
        assert_eq!(s.ids_for("alice").len(), 2);
        assert!(s.ids_for("bob").is_empty());
    }

    #[test]
    fn ticking_under_a_new_agent_replaces_rather_than_merges() {
        let mut s = selection_under("alice", &["a/one", "a/two"]);
        s.toggle("bob", "b/one");
        assert_eq!(s.len_for("bob"), 1);
        assert!(s.contains("bob", "b/one"));
        // Alice's ids are gone, not hiding behind the new stamp.
        assert!(s.ids_for("alice").is_empty());
        assert!(!s.contains("bob", "a/one"));
    }

    #[test]
    fn select_page_under_a_new_agent_also_replaces() {
        let mut s = selection_under("alice", &["a/one"]);
        s.extend("bob", ["b/one".to_string(), "b/two".to_string()]);
        assert_eq!(s.len_for("bob"), 2);
        assert!(s.ids_for("alice").is_empty());
    }

    #[test]
    fn deselect_page_narrows_the_current_set() {
        let mut s = selection_under("alice", &["a/one", "a/two", "a/three"]);
        s.remove_all(&["a/one".to_string(), "a/two".to_string()]);
        assert_eq!(s.len_for("alice"), 1);
        assert!(s.contains("alice", "a/three"));
    }

    #[test]
    fn clear_empties_it_for_every_agent() {
        let mut s = selection_under("alice", &["a/one"]);
        s.clear();
        assert_eq!(s.len_for("alice"), 0);
        assert!(s.ids_for("alice").is_empty());
    }
}
