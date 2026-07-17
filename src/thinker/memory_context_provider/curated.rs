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
        if let Some(snap) = self.curated_snapshots.read().await.get(&key) {
            return Ok(self.snapshot_to_message(snap));
        }
        let snap = Arc::new(self.capture_curated(agent_id).await?);
        self.curated_snapshots
            .write()
            .await
            .insert(key, snap.clone());
        Ok(self.snapshot_to_message(&snap))
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
            agent_id: agent_id.to_string(),
            agent_md_block: agent_block,
            user_md_block: user_block,
            open_loops_block,
            captured_at: std::time::SystemTime::now(),
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
        self.curated_snapshots
            .write()
            .await
            .retain(|(_, sk), _| sk != session_key);
        self.orientation_snapshots
            .write()
            .await
            .retain(|(_, sk), _| sk != session_key);
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
