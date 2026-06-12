//! local_voice tool — conversational status/warmup for the aleph-voice sidecar
//! (R8: 对话即管理面板).

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::gateway::voice::sidecar;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LocalVoiceAction {
    /// Report sidecar/model/engine state without spawning anything.
    Status,
    /// Spawn the sidecar (if needed) and pre-load models + engines.
    Warmup,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct LocalVoiceArgs {
    pub action: LocalVoiceAction,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalVoiceOutput {
    pub success: bool,
    pub message: String,
    /// Raw sidecar /v1/voice/status JSON when reachable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<serde_json::Value>,
}

#[derive(Clone, Default)]
pub struct LocalVoiceTool;

impl LocalVoiceTool {
    pub const fn new() -> Self {
        Self
    }

    pub async fn execute(&self, args: LocalVoiceArgs) -> LocalVoiceOutput {
        let Some(sup) = sidecar::global() else {
            return LocalVoiceOutput {
                success: false,
                message: "Local voice is disabled. Set [voice.local] enabled = true in config and restart.".into(),
                status: None,
            };
        };
        match args.action {
            LocalVoiceAction::Status => match sup.peek_endpoint().await {
                None => LocalVoiceOutput {
                    success: true,
                    message: "Sidecar not running (starts on first voice use or warmup). Models persist on disk.".into(),
                    status: None,
                },
                Some(ep) => {
                    let fetched = reqwest::Client::new()
                        .get(format!("{}/voice/status", ep.base_url))
                        .bearer_auth(&ep.token)
                        .timeout(std::time::Duration::from_secs(3))
                        .send()
                        .await;
                    match fetched {
                        Ok(resp) if resp.status().is_success() => {
                            let v: serde_json::Value = resp.json().await.unwrap_or_default();
                            LocalVoiceOutput {
                                success: true,
                                message: "Sidecar running.".into(),
                                status: Some(v),
                            }
                        }
                        other => {
                            let detail = match other {
                                Ok(resp) => format!("HTTP {}", resp.status()),
                                Err(e) => format!("{e}"),
                            };
                            LocalVoiceOutput {
                                success: false,
                                message: format!("Sidecar unreachable: {detail}"),
                                status: None,
                            }
                        }
                    }
                }
            },
            LocalVoiceAction::Warmup => match sup.warmup().await {
                Ok(()) => LocalVoiceOutput {
                    success: true,
                    message: "Warmup started: models downloading/loading in the background.".into(),
                    status: None,
                },
                Err(e) => LocalVoiceOutput {
                    success: false,
                    message: format!("Warmup failed: {e:#}"),
                    status: None,
                },
            },
        }
    }
}

#[async_trait]
impl AlephTool for LocalVoiceTool {
    const NAME: &'static str = "local_voice";
    const DESCRIPTION: &'static str = "Inspect or warm up the local voice (STT/TTS) sidecar. \
        Use action=status when the user asks about local voice readiness or model download progress; \
        action=warmup to pre-load models so the next voice interaction is instant.";

    type Args = LocalVoiceArgs;
    type Output = LocalVoiceOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            r#"local_voice(action="status")"#.to_string(),
            r#"local_voice(action="warmup")"#.to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        Ok(self.execute(args).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn disabled_state_is_friendly_not_fatal() {
        // No init_global in tests → graceful "disabled" message.
        let out = LocalVoiceTool::new()
            .execute(LocalVoiceArgs { action: LocalVoiceAction::Status })
            .await;
        assert!(!out.success);
        assert!(out.message.contains("voice.local"));
    }

    #[test]
    fn action_parses_lowercase() {
        let a: LocalVoiceArgs = serde_json::from_str(r#"{"action":"warmup"}"#).unwrap();
        assert_eq!(a.action, LocalVoiceAction::Warmup);
    }
}
