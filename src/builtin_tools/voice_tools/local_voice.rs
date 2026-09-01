//! local_voice tool — conversational status probe for the BYO local voice
//! endpoint (R8: 对话即管理面板).

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::config::Config;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LocalVoiceAction {
    /// Report the configured endpoint and probe its reachability.
    Status,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct LocalVoiceArgs {
    pub action: LocalVoiceAction,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalVoiceOutput {
    pub success: bool,
    pub message: String,
    /// Configuration summary + probe result when local voice is enabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<serde_json::Value>,
}

#[derive(Clone)]
pub struct LocalVoiceTool {
    config: Arc<RwLock<Config>>,
}

impl LocalVoiceTool {
    pub const fn new(config: Arc<RwLock<Config>>) -> Self {
        Self { config }
    }

    pub async fn execute(&self, args: LocalVoiceArgs) -> LocalVoiceOutput {
        let LocalVoiceAction::Status = args.action;
        let local = { self.config.read().await.local_voice().clone() };
        if !local.enabled {
            return LocalVoiceOutput {
                success: false,
                message: "Local voice is disabled. Set [voice.local] enabled = true and point \
                          endpoint at your OpenAI-compatible voice server (e.g. an mlx-audio \
                          server exposing /v1/audio/speech and /v1/audio/transcriptions), then \
                          restart."
                    .into(),
                status: None,
            };
        }

        let endpoint = local.endpoint.trim_end_matches('/').to_string();
        // Lightweight reachability probe: GET {endpoint}/models is implemented
        // by most OpenAI-compatible servers. 3 s is a generous window for
        // "is the local voice stack reachable" — slower than that and the
        // operator can already see the daemon is wedged.
        const REACHABILITY_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
        // Reuse a single client across calls so the keep-alive pool
        // applies (a fresh `Client::new` per probe pays full TLS+TCP
        // setup on every status check, which the operator's
        // reachability tests do not want to thrash).
        let client = match reqwest::Client::builder()
            .timeout(REACHABILITY_PROBE_TIMEOUT)
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                return LocalVoiceOutput {
                    success: false,
                    message: format!("failed to build HTTP client: {e}"),
                    status: Some(serde_json::json!({
                        "endpoint": endpoint,
                        "error": format!("client build failed: {e}"),
                    })),
                };
            }
        };
        let mut req = client.get(format!("{endpoint}/models"));
        if let Some(key) = local.api_key.as_deref().filter(|k| !k.is_empty()) {
            req = req.bearer_auth(key);
        }
        let probe = req.send().await;

        let or_default = |s: &str| {
            if s.is_empty() {
                "(server default)".to_string()
            } else {
                s.to_string()
            }
        };
        let summary = |reachable: bool, detail: &str| {
            serde_json::json!({
                "endpoint": endpoint,
                "reachable": reachable,
                "probe": detail,
                "stt_model": or_default(&local.stt_model),
                "tts_model": or_default(&local.tts_model),
                "tts_voice": or_default(&local.tts_voice),
                "tts_format": local.tts_format,
            })
        };

        match probe {
            Ok(resp) if resp.status().is_success() => LocalVoiceOutput {
                success: true,
                message: format!(
                    "Local voice endpoint {endpoint} is reachable. Voice requests will use it."
                ),
                status: Some(summary(true, "GET /models OK")),
            },
            other => {
                let detail = match other {
                    Ok(resp) => format!("GET /models returned HTTP {}", resp.status()),
                    Err(e) => format!("GET /models failed: {e}"),
                };
                LocalVoiceOutput {
                    success: false,
                    message: format!(
                        "Local voice endpoint {endpoint} is unreachable ({detail}). Make sure \
                         your OpenAI-compatible voice server is running there, or fix \
                         [voice.local] endpoint in config. Cloud transcription fallback (if \
                         configured) still applies."
                    ),
                    status: Some(summary(false, &detail)),
                }
            }
        }
    }
}

#[async_trait]
impl AlephTool for LocalVoiceTool {
    const NAME: &'static str = "local_voice";
    const DESCRIPTION: &'static str = "Check the local voice (BYO OpenAI-compatible STT/TTS \
        endpoint) configuration and reachability. Use action=status when the user asks whether \
        local voice is ready, configured, or why voice requests fail.";

    type Args = LocalVoiceArgs;
    type Output = LocalVoiceOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        Ok(self.execute(args).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_with(cfg: Config) -> LocalVoiceTool {
        LocalVoiceTool::new(Arc::new(RwLock::new(cfg)))
    }

    #[tokio::test]
    async fn disabled_state_is_friendly_not_fatal() {
        let out = tool_with(Config::default())
            .execute(LocalVoiceArgs {
                action: LocalVoiceAction::Status,
            })
            .await;
        assert!(!out.success);
        assert!(out.message.contains("voice.local"));
    }

    #[tokio::test]
    async fn unreachable_endpoint_reports_config_summary() {
        let mut cfg = Config::default();
        cfg.voice_local.local.enabled = true;
        // Reserved TEST-NET-1 address — connection fails fast within timeout.
        cfg.voice_local.local.endpoint = "http://192.0.2.1:1/v1".into();
        cfg.voice_local.local.tts_voice = "vivian".into();
        let out = tool_with(cfg)
            .execute(LocalVoiceArgs {
                action: LocalVoiceAction::Status,
            })
            .await;
        assert!(!out.success);
        assert!(out.message.contains("unreachable"));
        let status = out.status.expect("status summary");
        assert_eq!(status["reachable"], false);
        assert_eq!(status["endpoint"], "http://192.0.2.1:1/v1");
        assert_eq!(status["tts_voice"], "vivian");
        // Empty model names surface as "(server default)" for the LLM.
        assert_eq!(status["stt_model"], "(server default)");
    }

    #[test]
    fn action_parses_lowercase() {
        let a: LocalVoiceArgs = serde_json::from_str(r#"{"action":"status"}"#).unwrap();
        assert_eq!(a.action, LocalVoiceAction::Status);
    }
}
