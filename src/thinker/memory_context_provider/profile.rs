use super::{helpers, MemoryContextProvider};
use crate::config::types::memory::MemoryInjectionMode;
use crate::providers::message::UnifiedMessage;

impl MemoryContextProvider {
    /// Build a user-profile user-message for injection into the prompt.
    ///
    /// Returns `Ok(None)` when:
    /// - mode is `Tools`
    /// - no profile synthesizer is registered
    /// - the synthesizer returns `None` (USER.md absent)
    ///
    /// Otherwise returns `Ok(Some(UnifiedMessage::user(xml)))` with the
    /// profile envelope XML (body truncated to 2 KB).
    pub async fn build_profile_user_message(
        &self,
        agent_id: &str,
        mode: MemoryInjectionMode,
    ) -> Result<Option<UnifiedMessage>, crate::error::AlephError> {
        if matches!(mode, MemoryInjectionMode::Tools) {
            return Ok(None);
        }
        let Some(ps) = &self.profile else {
            return Ok(None);
        };
        let Some(profile) = ps.current(agent_id).await? else {
            return Ok(None);
        };
        let body = helpers::strip_frontmatter(&profile.raw);
        let body: String = body.chars().take(2048).collect();
        let xml = format!(
            "<UserProfile>\n<revision>{}</revision>\n<confidence>{}</confidence>\n<body>\n{}\n</body>\n</UserProfile>",
            profile.revision,
            helpers::xml_escape(&profile.confidence),
            helpers::xml_escape(&body)
        );
        Ok(Some(UnifiedMessage::user(xml)))
    }
}
