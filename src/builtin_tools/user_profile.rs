//! `user_profile` — read the current user profile or view revision history.

use crate::error::AlephError;
use crate::memory::notes::profile::synthesizer::ProfileSynthesizer;
use crate::sync_primitives::Arc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum UserProfileArgs {
    Read,
    History,
}

#[derive(Debug, Clone, Serialize)]
pub struct UserProfileOutput {
    pub content: Option<String>,
    pub revision: Option<u32>,
    pub confidence: Option<String>,
    pub history: Option<String>,
}

pub struct UserProfileTool {
    pub(crate) synthesizer: Arc<dyn ProfileSynthesizer>,
}

impl Clone for UserProfileTool {
    fn clone(&self) -> Self {
        Self {
            synthesizer: Arc::clone(&self.synthesizer),
        }
    }
}

impl UserProfileTool {
    /// Model-facing description — the single source for both the static
    /// catalog (`BUILTIN_TOOL_DEFINITIONS`) and the registry constructor.
    /// A catalog entry shadows whatever the constructor registers under the
    /// same name, so a second copy of this text anywhere is a copy the model
    /// never sees.
    pub const DESCRIPTION: &'static str =
        "Read the current user profile (interests, preferences, context) or view \
         its revision history. Use 'read' to get the latest profile, 'history' to \
         inspect the revision log.";

    pub fn new(synthesizer: Arc<dyn ProfileSynthesizer>) -> Self {
        Self { synthesizer }
    }

    pub async fn call(
        &self,
        agent_id: &str,
        args: UserProfileArgs,
    ) -> Result<UserProfileOutput, AlephError> {
        match args {
            UserProfileArgs::Read => {
                let profile = self.synthesizer.current(agent_id).await?;
                match profile {
                    Some(p) => Ok(UserProfileOutput {
                        content: Some(p.raw),
                        revision: Some(p.revision),
                        confidence: Some(p.confidence),
                        history: None,
                    }),
                    None => Ok(UserProfileOutput {
                        content: None,
                        revision: None,
                        confidence: None,
                        history: None,
                    }),
                }
            }
            UserProfileArgs::History => {
                let profile = self.synthesizer.current(agent_id).await?;
                Ok(UserProfileOutput {
                    content: None,
                    revision: profile.as_ref().map(|p| p.revision),
                    confidence: profile.as_ref().map(|p| p.confidence.clone()),
                    history: Some("Event-sourced history replay not yet implemented. Use revision number to track changes.".into()),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::notes::profile::types::{SessionSignal, UpdateOutcome, UserProfile};
    use crate::sync_primitives::Arc;
    use async_trait::async_trait;

    struct MockSynth;

    #[async_trait]
    impl ProfileSynthesizer for MockSynth {
        async fn bootstrap(&self, _: &str) -> Result<UserProfile, AlephError> {
            unimplemented!()
        }
        async fn current(&self, _: &str) -> Result<Option<UserProfile>, AlephError> {
            Ok(Some(UserProfile {
                schema_version: 1,
                updated: "2026-04-17".into(),
                revision: 5,
                last_session: "s1".into(),
                confidence: "high".into(),
                sections: Default::default(),
                sources: Default::default(),
                raw: "## Identity\n- test user".into(),
                content_hash: "abc".into(),
            }))
        }
        async fn update(&self, _: &str, _: SessionSignal) -> Result<UpdateOutcome, AlephError> {
            Ok(UpdateOutcome::Unchanged)
        }
    }

    #[tokio::test]
    async fn read_returns_profile() {
        let tool = UserProfileTool::new(Arc::new(MockSynth));
        let out = tool.call("default", UserProfileArgs::Read).await.unwrap();
        assert_eq!(out.revision, Some(5));
        assert!(out.content.unwrap().contains("test user"));
    }

    #[tokio::test]
    async fn history_returns_placeholder() {
        let tool = UserProfileTool::new(Arc::new(MockSynth));
        let out = tool
            .call("default", UserProfileArgs::History)
            .await
            .unwrap();
        assert!(out.history.unwrap().contains("not yet implemented"));
    }
}
