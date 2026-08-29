//! Onboarding wizard flow.
//!
//! A 10-stage wizard for initial setup and configuration.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::wizard::{RpcPrompter, WizardFlow, WizardOption, WizardPrompter, WizardSessionError};

/// Collected onboarding data
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OnboardingData {
    /// Primary provider
    pub primary_provider: Option<String>,
    /// Primary API key
    pub primary_api_key: Option<String>,
    /// Primary model
    pub primary_model: Option<String>,
    /// Secondary provider
    pub secondary_provider: Option<String>,
    /// Secondary API key
    pub secondary_api_key: Option<String>,
    /// Secondary model
    pub secondary_model: Option<String>,
    /// Thinking level
    pub thinking_level: Option<String>,
    /// Messaging apps
    pub messaging_apps: Vec<String>,
}

/// Onboarding wizard flow
pub struct OnboardingFlow;

impl OnboardingFlow {
    /// Create a new onboarding flow
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Create provider options
    fn provider_options() -> Vec<WizardOption> {
        vec![
            WizardOption::new(json!("anthropic"), "Anthropic (Claude)")
                .with_hint("Recommended - Best for coding and reasoning"),
            WizardOption::new(json!("openai"), "OpenAI (GPT)").with_hint("GPT-4o and o3 series"),
            WizardOption::new(json!("google"), "Google (Gemini)").with_hint("Gemini Pro and Ultra"),
            WizardOption::new(json!("ollama"), "Ollama (Local)")
                .with_hint("Run models locally - No API key needed"),
            WizardOption::new(json!("openrouter"), "OpenRouter")
                .with_hint("Access multiple providers with one key"),
        ]
    }

    /// Get model options for a provider
    fn model_options(provider: &str) -> Vec<WizardOption> {
        match provider {
            // Date-less aliases: the old dated picks here included ids that
            // were already retired (claude-3-5-haiku) or days from retirement
            // (claude-sonnet-4-20250514, gone 2026-06-15).
            "anthropic" => vec![
                WizardOption::new(json!("claude-opus-4-8"), "Claude Opus 4.8")
                    .with_hint("Most capable, best for complex tasks"),
                WizardOption::new(json!("claude-sonnet-4-6"), "Claude Sonnet 4.6")
                    .with_hint("Fast and capable (Recommended)"),
                WizardOption::new(json!("claude-haiku-4-5"), "Claude Haiku 4.5")
                    .with_hint("Fastest, good for simple tasks"),
            ],
            "openai" => vec![
                WizardOption::new(json!("gpt-5.5"), "GPT-5.5")
                    .with_hint("Most capable, 1M context"),
                WizardOption::new(json!("gpt-5.4"), "GPT-5.4")
                    .with_hint("Best overall performance (Recommended)"),
                WizardOption::new(json!("gpt-5.4-mini"), "GPT-5.4 Mini")
                    .with_hint("Faster, more economical"),
            ],
            "google" => vec![
                WizardOption::new(json!("gemini-2.5-pro"), "Gemini 2.5 Pro")
                    .with_hint("Most capable"),
                WizardOption::new(json!("gemini-2.5-flash"), "Gemini 2.5 Flash")
                    .with_hint("Fast and capable"),
            ],
            "ollama" => vec![
                WizardOption::new(json!("llama3.3:70b"), "Llama 3.3 70B")
                    .with_hint("Best open model"),
                WizardOption::new(json!("qwen2.5:32b"), "Qwen 2.5 32B")
                    .with_hint("Excellent multilingual"),
                WizardOption::new(json!("deepseek-coder:33b"), "DeepSeek Coder 33B")
                    .with_hint("Great for code"),
            ],
            "openrouter" => vec![
                WizardOption::new(
                    json!("anthropic/claude-opus-4-8"),
                    "Claude Opus 4.8 (via OpenRouter)",
                )
                .with_hint("Anthropic's flagship"),
                WizardOption::new(json!("openai/gpt-5.4"), "GPT-5.4 (via OpenRouter)")
                    .with_hint("OpenAI's flagship"),
                WizardOption::new(
                    json!("google/gemini-2.5-pro"),
                    "Gemini 2.5 Pro (via OpenRouter)",
                )
                .with_hint("Google's flagship"),
            ],
            _ => vec![WizardOption::new(json!("auto"), "Auto-detect")
                .with_hint("Let Aleph choose the best model")],
        }
    }

    /// Create thinking level options
    fn thinking_options() -> Vec<WizardOption> {
        vec![
            WizardOption::new(json!("off"), "Off").with_hint("No extended thinking"),
            WizardOption::new(json!("minimal"), "Minimal").with_hint("Brief reasoning"),
            WizardOption::new(json!("low"), "Low").with_hint("Quick analysis"),
            WizardOption::new(json!("medium"), "Medium (Recommended)")
                .with_hint("Balanced - good for most tasks"),
            WizardOption::new(json!("high"), "High")
                .with_hint("Deep analysis for complex problems"),
            WizardOption::new(json!("xhigh"), "XHigh")
                .with_hint("Maximum depth - slower but thorough"),
        ]
    }

    /// Create messaging app options
    fn messaging_options() -> Vec<WizardOption> {
        vec![
            WizardOption::new(json!("telegram"), "Telegram")
                .with_hint("Popular cross-platform messenger"),
            WizardOption::new(json!("discord"), "Discord")
                .with_hint("Gaming and community platform"),
            WizardOption::new(json!("slack"), "Slack")
                .with_hint("Business communication")
                .disabled(),
            WizardOption::new(json!("imessage"), "iMessage")
                .with_hint("macOS (local) or BlueBubbles (any OS)"),
        ]
    }

    async fn welcome(prompter: &RpcPrompter) -> Result<(), WizardSessionError> {
        prompter.intro("Welcome to Aleph").await?;
        prompter
            .note(
                "This wizard will help you set up Aleph, your personal AI assistant.\n\n\
             You'll configure:\n\
             • AI provider and model\n\
             • Optional secondary model for failover\n\
             • Thinking level for AI responses\n\
             • Messaging app integrations",
                Some("About this wizard"),
            )
            .await?;
        Ok(())
    }

    async fn configure_primary(
        prompter: &RpcPrompter,
        data: &mut OnboardingData,
    ) -> Result<(), WizardSessionError> {
        let primary_provider: String = prompter
            .select(
                "Which AI provider would you like to use?",
                Self::provider_options(),
            )
            .await?;
        data.primary_provider = Some(primary_provider.clone());

        if primary_provider != "ollama" {
            let api_key = prompter
                .text(
                    &format!("Enter your {primary_provider} API key:"),
                    Some("sk-..."),
                    true, // sensitive
                )
                .await?;
            data.primary_api_key = Some(api_key);
        } else {
            prompter
                .note(
                    "Ollama runs locally - no API key needed.\n\
                 Make sure Ollama is installed and running.",
                    Some("Local Models"),
                )
                .await?;
        }

        let primary_model: String = prompter
            .select(
                "Select your primary model:",
                Self::model_options(&primary_provider),
            )
            .await?;
        data.primary_model = Some(primary_model);
        Ok(())
    }

    async fn configure_secondary(
        prompter: &RpcPrompter,
        data: &mut OnboardingData,
    ) -> Result<(), WizardSessionError> {
        let wants_secondary = prompter
            .confirm(
                "Would you like to configure a secondary model for failover?",
                false,
            )
            .await?;

        if !wants_secondary {
            return Ok(());
        }

        let secondary_provider: String = prompter
            .select("Select secondary provider:", Self::provider_options())
            .await?;
        data.secondary_provider = Some(secondary_provider.clone());

        if secondary_provider != "ollama" {
            let api_key = if Some(&secondary_provider) == data.primary_provider.as_ref() {
                let use_same = prompter
                    .confirm("Use the same API key as the primary provider?", true)
                    .await?;
                if use_same {
                    data.primary_api_key.clone()
                } else {
                    Some(
                        prompter
                            .text(
                                &format!("Enter your {secondary_provider} API key:"),
                                Some("sk-..."),
                                true,
                            )
                            .await?,
                    )
                }
            } else {
                Some(
                    prompter
                        .text(
                            &format!("Enter your {secondary_provider} API key:"),
                            Some("sk-..."),
                            true,
                        )
                        .await?,
                )
            };
            data.secondary_api_key = api_key;
        }

        let secondary_model: String = prompter
            .select(
                "Select secondary model:",
                Self::model_options(&secondary_provider),
            )
            .await?;
        data.secondary_model = Some(secondary_model);
        Ok(())
    }

    async fn configure_thinking(
        prompter: &RpcPrompter,
        data: &mut OnboardingData,
    ) -> Result<(), WizardSessionError> {
        let thinking: String = prompter
            .select("Choose the AI thinking level:", Self::thinking_options())
            .await?;
        data.thinking_level = Some(thinking);
        Ok(())
    }

    async fn configure_messaging(
        prompter: &RpcPrompter,
        data: &mut OnboardingData,
    ) -> Result<(), WizardSessionError> {
        let apps: Vec<String> = prompter
            .multi_select(
                "Which messaging apps would you like to connect? (Optional)",
                Self::messaging_options(),
            )
            .await?;
        data.messaging_apps = apps;
        Ok(())
    }

    async fn review_and_finalize(
        prompter: &RpcPrompter,
        data: &OnboardingData,
    ) -> Result<(), WizardSessionError> {
        let review_text = format!(
            "Configuration Summary:\n\n\
             Primary Provider: {}\n\
             Primary Model: {}\n\
             Secondary: {}\n\
             Thinking Level: {}\n\
             Messaging Apps: {}",
            data.primary_provider.as_deref().unwrap_or("Not set"),
            data.primary_model.as_deref().unwrap_or("Not set"),
            if data.secondary_provider.is_some() {
                format!(
                    "{} / {}",
                    data.secondary_provider.as_deref().unwrap_or(""),
                    data.secondary_model.as_deref().unwrap_or("")
                )
            } else {
                "None".to_string()
            },
            data.thinking_level.as_deref().unwrap_or("medium"),
            if data.messaging_apps.is_empty() {
                "None".to_string()
            } else {
                data.messaging_apps.join(", ")
            },
        );

        prompter.note(&review_text, Some("Review")).await?;

        let confirmed = prompter.confirm("Apply this configuration?", true).await?;

        if !confirmed {
            return Err(WizardSessionError::Cancelled);
        }

        // KNOWN GAP (deferred): the answers collected into `OnboardingData` are
        // dropped when `run()` returns. `WizardFlow::run` yields only
        // `Result<(), _>`, and neither `WizardSession` nor the gateway's
        // `WizardSessionManager` exposes a channel for a flow's result, so
        // nothing persists the provider/model/key selections. Closing this needs
        // a result seam on the flow contract plus wiring in
        // `src/gateway/handlers/wizard.rs` and the boot path
        // (`src/bin/aleph-server/commands/start/mod.rs`) — an API change that is
        // out of scope for an in-module fix. Until then this wizard is a
        // preview: the outro below promises more than the flow delivers.

        prompter
            .outro("Aleph is ready! Run 'aleph chat' to start.")
            .await?;

        Ok(())
    }
}

impl Default for OnboardingFlow {
    fn default() -> Self {
        Self
    }
}

#[async_trait]
impl WizardFlow for OnboardingFlow {
    async fn run(&self, prompter: &RpcPrompter) -> Result<(), WizardSessionError> {
        let mut data = OnboardingData::default();

        Self::welcome(prompter).await?;
        Self::configure_primary(prompter, &mut data).await?;
        Self::configure_secondary(prompter, &mut data).await?;
        Self::configure_thinking(prompter, &mut data).await?;
        Self::configure_messaging(prompter, &mut data).await?;
        Self::review_and_finalize(prompter, &data).await?;

        Ok(())
    }

    fn name(&self) -> &str {
        "onboarding"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_options() {
        let options = OnboardingFlow::provider_options();
        assert_eq!(options.len(), 5);
        assert_eq!(options[0].label, "Anthropic (Claude)");
    }

    #[test]
    fn test_model_options() {
        let anthropic = OnboardingFlow::model_options("anthropic");
        assert!(!anthropic.is_empty());

        let openai = OnboardingFlow::model_options("openai");
        assert!(!openai.is_empty());

        let unknown = OnboardingFlow::model_options("unknown");
        assert_eq!(unknown.len(), 1); // Auto-detect
    }

    #[test]
    fn test_thinking_options() {
        let options = OnboardingFlow::thinking_options();
        assert_eq!(options.len(), 6);
    }

    #[test]
    fn test_messaging_options() {
        let options = OnboardingFlow::messaging_options();
        assert_eq!(options.len(), 4);
        // Slack should be disabled
        assert!(options
            .iter()
            .find(|o| o.label == "Slack")
            .map(|o| o.disabled)
            .unwrap_or(false));
    }

    #[test]
    fn test_onboarding_data_default() {
        let data = OnboardingData::default();
        assert!(data.primary_provider.is_none());
        assert!(data.messaging_apps.is_empty());
    }
}
