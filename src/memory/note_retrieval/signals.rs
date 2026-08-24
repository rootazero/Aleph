//! Recall-signal writes and the reinforcement counts they feed back into
//! scoring. Each has an owner-keyed twin, because a note's signals are filed
//! under the partition that owns the note, not the one that asked.
//!
//! Split out of `note_retrieval/mod.rs` verbatim; logic unchanged. These are
//! inherent methods, so they need no delegation layer — the type simply has
//! more than one `impl` block.

use super::*;

impl<S: NoteStore + Send + Sync + 'static> NoteFactRetrieval<S> {
    /// Producer of the `recall_signals` table — the raw access signal three
    /// independent consumers depend on: hot-floating reinforcement ranking
    /// (`fetch_reinforcement_counts`), `NoteDecay`'s `access_weight`, and the
    /// evolution recall-evidence gate (`recall_hit_counts`). Recording is
    /// therefore **unconditional** (only skipped on an empty result set): it is
    /// NOT gated on `reinforcement_enabled`, because disabling hot-floating
    /// ranking must not silently blind decay and the evolution gate to which
    /// notes are actually being used (which made every note look never-recalled
    /// and biased archival). The reinforcement *ranking boost* stays gated in
    /// `apply_scoring`. Best-effort — a write failure never breaks recall. The
    /// store dedups per `(note_path, query, day, channel)`, so a note must
    /// surface across *distinct* queries/days to genuinely heat up.
    pub(super) async fn record_recall(&self, query: &str, agent_id: &str, ranked: &[ScoredFact]) {
        if ranked.is_empty() {
            return;
        }
        let hits: Vec<(String, f32)> = ranked
            .iter()
            // rust-doctor-disable-next-line excessive-clone
            .map(|f| (f.fact.id.clone(), f.score))
            .collect();
        if let Err(e) = self
            .indexer
            .store()
            .record_recall_hits(query, AUTO_RECALL_CHANNEL, &hits, agent_id)
            .await
        {
            tracing::debug!(error = %e, "failed to record recall signals (non-fatal)");
        }
    }

    /// Bucket `ranked` by the note's true owning namespace and record each
    /// bucket under that owner.
    ///
    /// `recall_signals` rows are per-agent, and every consumer reads them under
    /// a *specific* agent id: `NoteDecay`'s `access_weight` and the evolution
    /// recall-evidence gate both run with the dream context's **scoped** id.
    /// Filing a project note's hit under the base namespace therefore makes the
    /// note look never-recalled to decay (early archival) while crediting a
    /// base-namespace note that was never surfaced.
    ///
    /// `to_scored_fact(agent_id)` already stamped the true owner onto
    /// `fact.agent` when the results were collected, so no new plumbing is
    /// needed — just group by it.
    pub(super) async fn record_recall_by_owner(&self, query: &str, ranked: &[ScoredFact]) {
        let mut by_owner: HashMap<&str, Vec<ScoredFact>> = HashMap::new();
        for f in ranked {
            by_owner
                .entry(f.fact.agent.as_str())
                .or_default()
                .push(f.clone());
        }
        for (owner, bucket) in &by_owner {
            self.record_recall(query, owner, bucket).await;
        }
    }

    /// Fetch recall-frequency counts for the candidate notes when reinforcement
    /// salience is enabled. Returns an empty map when disabled or on any store
    /// error, degrading gracefully to neutral (legacy) scoring.
    pub(super) async fn fetch_reinforcement_counts(
        &self,
        agent_id: &str,
        facts: &[ScoredFact],
    ) -> HashMap<String, i64> {
        if !self.scoring.reinforcement_enabled || facts.is_empty() {
            return HashMap::new();
        }
        // rust-doctor-disable-next-line excessive-clone
        let paths: Vec<String> = facts.iter().map(|f| f.fact.id.clone()).collect();
        self.indexer
            .store()
            .recall_hit_counts(agent_id, &paths)
            .await
            .unwrap_or_default()
    }

    /// Reinforcement counts for a multi-owner candidate set, fetched per owning
    /// namespace and merged. The single-owner `fetch_reinforcement_counts` reads
    /// every path under one agent id, which returns zero for notes owned by any
    /// other namespace in the scope union — i.e. exactly the project notes the
    /// scoped read exists to surface.
    ///
    /// Keyed by `(owner, path)` because two namespaces can hold notes at the
    /// same relative path; a bare-path map would collapse them.
    pub(super) async fn fetch_reinforcement_counts_by_owner(
        &self,
        facts: &[ScoredFact],
    ) -> HashMap<(String, String), i64> {
        let mut by_owner: HashMap<&str, Vec<ScoredFact>> = HashMap::new();
        for f in facts {
            by_owner
                .entry(f.fact.agent.as_str())
                .or_default()
                .push(f.clone());
        }
        let mut merged: HashMap<(String, String), i64> = HashMap::new();
        for (owner, bucket) in &by_owner {
            let counts = self.fetch_reinforcement_counts(owner, bucket).await;
            for (path, n) in counts {
                merged.insert(((*owner).to_string(), path), n);
            }
        }
        merged
    }
}
