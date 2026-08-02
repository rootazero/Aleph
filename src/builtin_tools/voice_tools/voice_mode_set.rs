//! Voice mode set tool — toggle voice output on/off for a channel.
//!
//! Implements R9: Everything is a Tool. The LLM selects this tool when the
//! user says things like "turn on voice mode" or "switch to voice replies".

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::Result;
use crate::gateway::channel_registry::ChannelRegistry;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

// =============================================================================
// Args
// =============================================================================

/// Arguments for the `voice_mode_set` tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct VoiceModeSetArgs {
    /// Whether to enable (true) or disable (false) voice mode
    pub enabled: bool,

    /// Channel ID to apply voice mode to. Defaults to the current channel.
    #[serde(default)]
    pub channel_id: Option<String>,

    /// TTS provider override (e.g., "openai"). Keeps existing provider if omitted.
    #[serde(default)]
    pub provider: Option<String>,

    /// Voice ID override (e.g., "alloy"). Keeps existing voice if omitted.
    #[serde(default)]
    pub voice: Option<String>,
}

// =============================================================================
// Output
// =============================================================================

/// Output from `voice_mode_set` tool
#[derive(Debug, Clone, Serialize)]
pub struct VoiceModeSetOutput {
    /// Whether the operation succeeded
    pub success: bool,
    /// Channel ID that was updated
    pub channel_id: String,
    /// Current enabled state after the operation
    pub enabled: bool,
    /// Human-readable status message
    pub message: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that enables or disables voice mode for a channel.
///
/// Voice mode causes the `ReplyEmitter` to synthesise TTS audio for every
/// outbound text reply on the channel. Provider and voice can optionally be
/// overridden per-call.
#[derive(Clone)]
pub struct VoiceModeSetTool {
    channel_registry: Arc<ChannelRegistry>,
}

impl VoiceModeSetTool {
    /// Create a new `VoiceModeSetTool` backed by the given registry.
    pub const fn new(channel_registry: Arc<ChannelRegistry>) -> Self {
        Self { channel_registry }
    }

    /// Execute voice mode toggle logic.
    ///
    /// `current_channel_id` is the channel that originated the current message,
    /// used as fallback when `args.channel_id` is absent.
    pub async fn execute(
        &self,
        args: VoiceModeSetArgs,
        current_channel_id: Option<&str>,
    ) -> VoiceModeSetOutput {
        // Resolve target channel: args > current > "default"
        let channel_id = args
            .channel_id
            .clone()
            .or_else(|| current_channel_id.map(str::to_string))
            .unwrap_or_else(|| "default".to_string());

        let enabled = args.enabled;
        let provider_override = args.provider.clone();
        let voice_override = args.voice.clone();

        // Mutate voice state via registry
        self.channel_registry
            .update_voice_state(&channel_id, |state| {
                state.enabled = enabled;
                if let Some(p) = provider_override {
                    state.provider = Some(p);
                }
                if let Some(v) = voice_override {
                    state.voice = Some(v);
                }
                // Reset failure counter when explicitly toggling
                state.consecutive_failures = 0;
            })
            .await;

        let action = if enabled { "enabled" } else { "disabled" };
        let message = format!("Voice mode {action} for channel '{channel_id}'.");

        info!(channel_id = %channel_id, enabled = enabled, "Voice mode set via tool");

        VoiceModeSetOutput {
            success: true,
            channel_id,
            enabled,
            message,
        }
    }
}

#[async_trait]
impl AlephTool for VoiceModeSetTool {
    const NAME: &'static str = "voice_mode_set";
    const DESCRIPTION: &'static str = "Enable or disable voice mode for a channel. \
        When voice mode is on, replies are synthesised as speech audio in addition to text. \
        Use this when the user asks to 'turn on voice', 'switch to voice replies', \
        'disable voice', or similar requests.";

    type Args = VoiceModeSetArgs;
    type Output = VoiceModeSetOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // When called through the standard AlephTool path there is no
        // current_channel_id context — callers that have it should use
        // `execute()` directly.
        Ok(self.execute(args, None).await)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_registry() -> Arc<ChannelRegistry> {
        Arc::new(ChannelRegistry::new())
    }

    #[tokio::test]
    async fn test_enable_voice_mode() {
        let registry = make_registry();
        let tool = VoiceModeSetTool::new(Arc::clone(&registry));

        let args = VoiceModeSetArgs {
            enabled: true,
            channel_id: Some("telegram".to_string()),
            provider: None,
            voice: None,
        };

        let output = tool.execute(args, None).await;

        assert!(output.success);
        assert!(output.enabled);
        assert_eq!(output.channel_id, "telegram");
        assert!(output.message.contains("enabled"));

        // Verify state was written to registry
        let state = registry.get_voice_state("telegram").await;
        assert!(state.enabled);
        assert_eq!(state.consecutive_failures, 0);
    }

    #[tokio::test]
    async fn test_disable_voice_mode() {
        let registry = make_registry();
        let tool = VoiceModeSetTool::new(Arc::clone(&registry));

        // Enable first
        let enable_args = VoiceModeSetArgs {
            enabled: true,
            channel_id: Some("telegram".to_string()),
            provider: Some("openai".to_string()),
            voice: Some("alloy".to_string()),
        };
        tool.execute(enable_args, None).await;

        // Then disable
        let disable_args = VoiceModeSetArgs {
            enabled: false,
            channel_id: Some("telegram".to_string()),
            provider: None,
            voice: None,
        };
        let output = tool.execute(disable_args, None).await;

        assert!(output.success);
        assert!(!output.enabled);
        assert_eq!(output.channel_id, "telegram");
        assert!(output.message.contains("disabled"));

        let state = registry.get_voice_state("telegram").await;
        assert!(!state.enabled);
        // Provider/voice are preserved from enable call
        assert_eq!(state.provider, Some("openai".to_string()));
    }

    #[tokio::test]
    async fn test_channel_id_defaults_to_current_channel() {
        let registry = make_registry();
        let tool = VoiceModeSetTool::new(Arc::clone(&registry));

        let args = VoiceModeSetArgs {
            enabled: true,
            channel_id: None,
            provider: None,
            voice: None,
        };

        let output = tool.execute(args, Some("webchat-1")).await;

        assert!(output.success);
        assert_eq!(output.channel_id, "webchat-1");

        let state = registry.get_voice_state("webchat-1").await;
        assert!(state.enabled);
    }

    #[tokio::test]
    async fn test_channel_id_defaults_to_default_when_no_current() {
        let registry = make_registry();
        let tool = VoiceModeSetTool::new(Arc::clone(&registry));

        let args = VoiceModeSetArgs {
            enabled: true,
            channel_id: None,
            provider: None,
            voice: None,
        };

        let output = tool.execute(args, None).await;

        assert!(output.success);
        assert_eq!(output.channel_id, "default");
    }

    #[tokio::test]
    async fn test_provider_and_voice_override() {
        let registry = make_registry();
        let tool = VoiceModeSetTool::new(Arc::clone(&registry));

        let args = VoiceModeSetArgs {
            enabled: true,
            channel_id: Some("cli".to_string()),
            provider: Some("elevenlabs".to_string()),
            voice: Some("rachel".to_string()),
        };

        let output = tool.execute(args, None).await;

        assert!(output.success);

        let state = registry.get_voice_state("cli").await;
        assert!(state.enabled);
        assert_eq!(state.provider, Some("elevenlabs".to_string()));
        assert_eq!(state.voice, Some("rachel".to_string()));
    }

    #[tokio::test]
    async fn test_failure_counter_reset_on_toggle() {
        let registry = make_registry();

        // Manually set a state with failures
        registry
            .update_voice_state("chan", |s| {
                s.enabled = true;
                s.consecutive_failures = 2;
            })
            .await;

        let tool = VoiceModeSetTool::new(Arc::clone(&registry));
        let args = VoiceModeSetArgs {
            enabled: true,
            channel_id: Some("chan".to_string()),
            provider: None,
            voice: None,
        };
        tool.execute(args, None).await;

        let state = registry.get_voice_state("chan").await;
        assert_eq!(state.consecutive_failures, 0, "failures should be reset");
    }

    #[tokio::test]
    async fn test_aleph_tool_call_without_context() {
        let registry = make_registry();
        let tool = VoiceModeSetTool::new(registry);

        let args = VoiceModeSetArgs {
            enabled: true,
            channel_id: Some("any".to_string()),
            provider: None,
            voice: None,
        };

        let result = AlephTool::call(&tool, args).await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.success);
        assert!(output.enabled);
    }
}
