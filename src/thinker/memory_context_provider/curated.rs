use super::MemoryContextProvider;
use crate::memory::curated::{CuratedMemoryStore, CuratedSnapshot};
use crate::sync_primitives::Arc;

impl MemoryContextProvider {
    /// Build the cached `CuratedMemory` + `UserProfile` envelope for
    /// this `(agent_id, session_key)`. The first call captures a frozen
    /// snapshot; subsequent calls in the same session reuse it. Returns
    /// `Ok(None)` only when both blocks are empty (so the layer can skip
    /// emitting an empty user message).
    pub async fn build_curated_message(
        &self,
        agent_id: &str,
        session_key: &str,
    ) -> Result<Option<crate::providers::message::UnifiedMessage>, crate::error::AlephError> {
        let key = (agent_id.to_string(), session_key.to_string());
        if let Some(entry) = self.curated_snapshots.read().await.get(&key) {
            return Ok(self.snapshot_to_message(&entry.value));
        }
        let snap = Arc::new(self.capture_curated(agent_id).await?);
        // Single write-guard get-or-insert: a concurrent first build that lost
        // the race gets the winner's snapshot back, so both render identical
        // bytes instead of one silently replacing the other's frozen envelope.
        let frozen = super::freeze_into(&mut *self.curated_snapshots.write().await, key, snap);
        Ok(self.snapshot_to_message(&frozen))
    }

    /// Get or lazily load the per-agent `CuratedMemoryStore`. Public so the
    /// builtin `remember` tool can resolve the same store instance the
    /// `CuratedMemoryLayer` renders into the system prompt.
    pub async fn get_or_load_curated_store(
        &self,
        agent_id: &str,
    ) -> Result<Arc<CuratedMemoryStore>, crate::error::AlephError> {
        if let Some(s) = self.curated_stores.get(agent_id) {
            return Ok(s.clone());
        }
        let path = self.agent_memory_path(agent_id);
        let s = Arc::new(
            CuratedMemoryStore::load(path, self.curated_config.memory_char_limit, agent_id).await?,
        );
        self.curated_stores.insert(agent_id.to_string(), s.clone());
        Ok(s)
    }

    /// Load (or reuse) the per-agent `CuratedMemoryStore`, render the
    /// `CuratedMemory` and `UserProfile` blocks, and return them as a
    /// frozen `CuratedSnapshot`.
    async fn capture_curated(
        &self,
        agent_id: &str,
    ) -> Result<CuratedSnapshot, crate::error::AlephError> {
        use crate::memory::curated::snapshot::{render_agent_block, render_user_block};

        let store = self.get_or_load_curated_store(agent_id).await?;

        let entries = store.current_entries();
        let agent_block = render_agent_block(
            &entries,
            self.curated_config.memory_char_limit,
            self.curated_config.legacy_warn_threshold,
        );

        let user_block = if let Some(ps) = &self.profile {
            match ps.current(agent_id).await? {
                Some(p) => {
                    let body = super::helpers::strip_frontmatter(&p.raw);
                    let block = render_user_block(
                        body,
                        self.curated_config.user_char_limit,
                        self.curated_config.legacy_warn_threshold,
                    );
                    if block.is_empty() {
                        None
                    } else {
                        Some(block)
                    }
                }
                None => None,
            }
        } else {
            None
        };

        // Open loops from the previous session (Batch 2 open-loop tracking).
        // Opt-in via `set_open_loop_inject`. `SessionReflector` writes
        // `OPEN_LOOPS.md` beside MEMORY.md on session end; absence (or the
        // feature being off) simply yields no block. Bounded to keep the
        // injected prompt small.
        const OPEN_LOOPS_CHAR_LIMIT: usize = 2000;
        let open_loops_block = if super::helpers::open_loop_inject() {
            let path = self
                .agent_memory_path(agent_id)
                .with_file_name("OPEN_LOOPS.md");
            match tokio::fs::read_to_string(&path).await {
                Ok(body) => {
                    let block = crate::memory::curated::snapshot::render_open_loops_block(
                        &body,
                        OPEN_LOOPS_CHAR_LIMIT,
                    );
                    (!block.is_empty()).then_some(block)
                }
                Err(_) => None,
            }
        } else {
            None
        };

        Ok(CuratedSnapshot {
            agent_md_block: agent_block,
            user_md_block: user_block,
            open_loops_block,
        })
    }

    /// Combine the two block strings into a single user-message. Returns
    /// `None` when both blocks are empty (no need to emit an empty turn).
    fn snapshot_to_message(
        &self,
        snap: &CuratedSnapshot,
    ) -> Option<crate::providers::message::UnifiedMessage> {
        let mut combined = String::new();
        if !snap.agent_md_block.is_empty() {
            combined.push_str(&snap.agent_md_block);
        }
        if let Some(ub) = &snap.user_md_block {
            if !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str(ub);
        }
        if let Some(ol) = &snap.open_loops_block {
            if !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str(ol);
        }
        if combined.is_empty() {
            return None;
        }
        Some(crate::providers::message::UnifiedMessage::user(combined))
    }

    /// Evict every snapshot whose `session_key` matches. Called on
    /// compression-complete and `SessionEnd` so the next prompt build picks
    /// up disk mutations. Evicts the frozen orientation envelope too — it
    /// rides the same Stable curated zone and must refresh at the same points.
    pub async fn invalidate_curated(&self, session_key: &str) {
        let agents: Vec<String> = {
            let mut snaps = self.curated_snapshots.write().await;
            // Collect the owning agents BEFORE the retain: the store eviction
            // below must be scoped to the agents this session actually
            // rendered, not to every agent the process has ever loaded.
            let agents = snaps
                .keys()
                .filter(|(_, sk)| sk == session_key)
                .map(|(aid, _)| aid.clone())
                .collect();
            snaps.retain(|(_, sk), _| sk != session_key);
            agents
        };
        self.orientation_snapshots
            .write()
            .await
            .retain(|(_, sk), _| sk != session_key);
        // Dropping the snapshot alone re-renders the SAME entries: the per-agent
        // `CuratedMemoryStore` holds the body it was loaded from, and
        // `capture_curated` reads `store.current_entries()` off that in-memory
        // state (only the write path ever re-reads disk). So a hand-edited
        // `~/.aleph/agents/<id>/MEMORY.md` — a supported legacy input — would
        // stay invisible until a compression run or a daemon restart, which is
        // exactly what this method's contract promises it does not. Mirrors
        // `invalidate_curated_for_agent`, which has always done this.
        for agent_id in agents {
            self.curated_stores.remove(&agent_id);
        }
    }

    /// Drop every cached curated snapshot for `agent_id` across every
    /// `session_key`. Spec A Task 18: fired after compression-run completes
    /// for the agent, since compression mutates `MEMORY.md` / `USER.md` on
    /// disk and any per-session cache must rebuild on the next prompt.
    pub async fn invalidate_curated_for_agent(&self, agent_id: &str) {
        self.curated_snapshots
            .write()
            .await
            .retain(|(aid, _), _| aid != agent_id);
        self.orientation_snapshots
            .write()
            .await
            .retain(|(aid, _), _| aid != agent_id);
        // The per-agent CuratedMemoryStore caches the last-loaded body in
        // memory; drop it too so the next `get_or_load_curated_store` call
        // re-reads MEMORY.md from disk.
        self.curated_stores.remove(agent_id);
    }
}

#[cfg(test)]
mod tests {
    use super::MemoryContextProvider;
    use crate::config::types::memory::MemoryInjectionMode;
    use crate::memory::curated::CuratedConfig;

    fn provider_rooted_at(root: &std::path::Path) -> MemoryContextProvider {
        MemoryContextProvider::new_for_test_empty_envelope(MemoryInjectionMode::Context)
            .with_curated_config(CuratedConfig {
                memory_char_limit: 200,
                user_char_limit: 200,
                legacy_warn_threshold: 0.95,
            })
            .with_curated_root_for_test(root.to_path_buf())
    }

    #[tokio::test]
    async fn session_end_invalidation_picks_up_a_hand_edited_memory_md() {
        // The scenario the doc-comment promises: a user hand-edits
        // MEMORY.md between sessions. Evicting only the snapshot re-renders
        // `store.current_entries()` — the body loaded once, at first capture —
        // so the edit stayed invisible until a compression run or a restart.
        let dir = tempfile::tempdir().unwrap();
        let provider = provider_rooted_at(dir.path());

        let agent_dir = dir.path().join("agent-x");
        tokio::fs::create_dir_all(&agent_dir).await.unwrap();
        tokio::fs::write(agent_dir.join("MEMORY.md"), "fact one\n§\n")
            .await
            .unwrap();

        let first = provider
            .build_curated_message("agent-x", "ses-1")
            .await
            .unwrap();
        assert!(format!("{first:?}").contains("fact one"));

        // Hand edit between sessions.
        tokio::fs::write(agent_dir.join("MEMORY.md"), "fact one\n§\nhand edited\n§\n")
            .await
            .unwrap();

        // Session end only — no manual store surgery, which is the whole point.
        provider.invalidate_curated("ses-1").await;

        let second = provider
            .build_curated_message("agent-x", "ses-2")
            .await
            .unwrap();
        assert!(
            format!("{second:?}").contains("hand edited"),
            "session-end eviction must drop the per-agent store too: {second:?}"
        );
    }

    #[tokio::test]
    async fn session_end_invalidation_leaves_other_agents_stores_alone() {
        // Scoping guard: evicting one session must not force every other agent
        // in the process to re-read MEMORY.md from disk.
        let dir = tempfile::tempdir().unwrap();
        let provider = provider_rooted_at(dir.path());

        for agent in ["agent-A", "agent-B"] {
            let d = dir.path().join(agent);
            tokio::fs::create_dir_all(&d).await.unwrap();
            tokio::fs::write(d.join("MEMORY.md"), format!("{agent} original\n§\n"))
                .await
                .unwrap();
        }
        provider
            .build_curated_message("agent-A", "ses-1")
            .await
            .unwrap();
        provider
            .build_curated_message("agent-B", "ses-2")
            .await
            .unwrap();

        // Mutate B's file, then end A's session.
        tokio::fs::write(
            dir.path().join("agent-B").join("MEMORY.md"),
            "agent-B rewritten\n§\n",
        )
        .await
        .unwrap();
        provider.invalidate_curated("ses-1").await;

        // B's store survives, so a fresh session for B still renders the body
        // it was loaded with (B's own eviction points are what refresh it).
        let b = provider
            .build_curated_message("agent-B", "ses-3")
            .await
            .unwrap();
        assert!(
            format!("{b:?}").contains("agent-B original"),
            "agent-B's store must not be evicted by agent-A's session end: {b:?}"
        );
    }

    #[test]
    fn freeze_into_bounds_the_map() {
        // Sessions that never close (a channel DM peer) used to pin an entry
        // for the process lifetime; the map had no ceiling at all.
        use super::super::{freeze_into, MAX_FROZEN_SNAPSHOTS};
        use std::collections::HashMap;

        let mut map = HashMap::new();
        for i in 0..MAX_FROZEN_SNAPSHOTS + 10 {
            freeze_into(&mut map, ("a".to_string(), format!("ses-{i}")), i);
        }
        assert!(
            map.len() <= MAX_FROZEN_SNAPSHOTS,
            "cache must stay bounded, got {}",
            map.len()
        );
    }

    #[test]
    fn freeze_into_keeps_the_first_write() {
        // "Frozen for the session" is what keeps the provider prompt-cache
        // prefix byte-stable: a capture that lost the race must read the
        // winner's value back, not replace it.
        use super::super::freeze_into;
        use std::collections::HashMap;

        let mut map = HashMap::new();
        let key = ("a".to_string(), "ses-1".to_string());
        assert_eq!(freeze_into(&mut map, key.clone(), 1), 1);
        assert_eq!(
            freeze_into(&mut map, key, 2),
            1,
            "the loser must reuse the frozen value, not overwrite it"
        );
    }
}
