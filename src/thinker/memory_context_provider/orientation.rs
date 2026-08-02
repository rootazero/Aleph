use super::MemoryContextProvider;
use crate::config::types::memory::MemoryInjectionMode;
use crate::providers::message::UnifiedMessage;

impl MemoryContextProvider {
    /// Build a wiki orientation user-message for injection into the prompt.
    ///
    /// Returns `Ok(None)` when:
    /// - mode is `Tools` (orientation is prompt-only, not tool-gated)
    /// - no wiki provider is registered
    /// - the snapshot is entirely empty (a notes-less agent must not receive
    ///   a contentless `<NoteOrientation>` XML skeleton every prompt — mirrors
    ///   `snapshot_to_message`'s emptiness contract)
    ///
    /// Otherwise returns `Ok(Some(UnifiedMessage::user(xml)))` with the
    /// orientation envelope XML.
    pub async fn build_orientation_user_message(
        &self,
        agent_id: &str,
        mode: MemoryInjectionMode,
    ) -> Result<Option<UnifiedMessage>, crate::error::AlephError> {
        if matches!(mode, MemoryInjectionMode::Tools) {
            return Ok(None);
        }
        let Some(w) = &self.orientation else {
            return Ok(None);
        };
        let snap = w.read_snapshot(agent_id, self.orientation_budget).await?;
        if snap.schema_text.trim().is_empty()
            && snap.index_text.trim().is_empty()
            && snap.recent_log_tail.trim().is_empty()
        {
            return Ok(None);
        }
        let xml = super::helpers::render_orientation_envelope(&snap);
        Ok(Some(UnifiedMessage::user(xml)))
    }

    /// Like [`Self::build_orientation_user_message`] but frozen per
    /// `(agent_id, session_key)`, mirroring the curated snapshot cache.
    ///
    /// Orientation rides the Stable curated envelope (`CuratedMemoryLayer`),
    /// which exists precisely to keep the cacheable prompt prefix byte-stable
    /// across a session. Re-reading the wiki from disk on every prompt build
    /// would re-key the provider prompt cache whenever the wiki mutated
    /// mid-session. The first build for a session captures the envelope (or
    /// its absence); [`Self::invalidate_curated`] /
    /// [`Self::invalidate_curated_for_agent`] evict it at the same points as
    /// the curated snapshot (session end / post-compression).
    pub async fn build_orientation_message_cached(
        &self,
        agent_id: &str,
        session_key: &str,
        mode: MemoryInjectionMode,
    ) -> Result<Option<UnifiedMessage>, crate::error::AlephError> {
        if matches!(mode, MemoryInjectionMode::Tools) {
            return Ok(None);
        }
        // No wiki handle → nothing to read or cache.
        if self.orientation.is_none() {
            return Ok(None);
        }
        let key = (agent_id.to_string(), session_key.to_string());
        if let Some(cached) = self.orientation_snapshots.read().await.get(&key) {
            return Ok(cached.value.clone().map(UnifiedMessage::user));
        }
        let text = self
            .build_orientation_user_message(agent_id, mode)
            .await?
            .as_ref()
            .map(UnifiedMessage::text_content);
        // Single write-guard get-or-insert (bounded + first-write-wins); see
        // `super::freeze_into`.
        let frozen = super::freeze_into(&mut *self.orientation_snapshots.write().await, key, text);
        Ok(frozen.map(UnifiedMessage::user))
    }
}
