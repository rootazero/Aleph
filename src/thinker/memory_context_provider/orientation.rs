use super::MemoryContextProvider;
use crate::config::types::memory::MemoryInjectionMode;
use crate::providers::message::UnifiedMessage;

impl MemoryContextProvider {
    /// Build a wiki orientation user-message for injection into the prompt.
    ///
    /// Returns `Ok(None)` when:
    /// - mode is `Tools` (orientation is prompt-only, not tool-gated)
    /// - no wiki provider is registered
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
        let xml = super::helpers::render_orientation_envelope(&snap);
        Ok(Some(UnifiedMessage::user(xml)))
    }
}
