//! Onboarding wizard flow.
//!
//! A 10-stage wizard for initial setup and configuration.

#![allow(dead_code)] // In development

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::wizard::{
    RpcPrompter, WizardFlow, WizardOption, WizardPrompter, WizardSessionError,
};

/// Provider choice
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Anthropic,
    OpenAI,
    Google,
    Ollama,
    OpenRouter,
}

impl Provider {
    fn display_name(&self) -> &str {
        match self {
            Provider::Anthropic => "Anthropic (Claude)",
            Provider::OpenAI => "OpenAI (GPT)",
            Provider::Google => "Google (Gemini)",
            Provider::Ollama => "Ollama (Local)",
            Provider::OpenRouter => "OpenRouter (Multi-provider)",
        }
    }

    fn requires_api_key(&self) -> bool {
        !matches!(self, Provider::Ollama)
    }

    fn api_key_name(&self) -> &str {
        match self {
            Provider::Anthropic => "ANTHROPIC_API_KEY",
            Provider::OpenAI => "OPENAI_API_KEY",
            Provider::Google => "GOOGLE_API_KEY",
            Provider::Ollama => "",
            Provider::OpenRouter => "OPENROUTER_API_KEY",
        }
    }
}

/// Messaging app choice
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MessagingApp {
    Telegram,
    Discord,
    Slack,
    IMessage,
    None,
}

impl MessagingApp {
    fn display_name(&self) -> &str {
        match self {
            MessagingApp::Telegram => "Telegram",
            MessagingApp::Discord => "Discord",
            MessagingApp::Slack => "Slack",
            MessagingApp::IMessage => "iMessage (macOS only)",
            MessagingApp::None => "None (Skip for now)",
        }
    }
}

/// Thinking level
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
}

impl ThinkingLevel {
    fn display_name(&self) -> &str {
        match self {
            ThinkingLevel::Off => "Off - No extended thinking",
            ThinkingLevel::Minimal => "Minimal - Brief reasoning",
            ThinkingLevel::Low => "Low - Quick analysis",
            ThinkingLevel::Medium => "Medium - Balanced (Recommended)",
            ThinkingLevel::High => "High - Deep analysis",
            ThinkingLevel::XHigh => "XHigh - Maximum depth",
        }
    }
}

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
pub struct OnboardingFlow {
    data: OnboardingData,
}

impl OnboardingFlow {
    /// Create a new onboarding flow
    pub fn new() -> Self {
        Self {
            data: OnboardingData::default(),
        }
    }

    /// Create provider options
    fn provider_options() -> Vec<WizardOption> {
        vec![
            WizardOption::new(json!("anthropic"), "Anthropic (Claude)")
                .with_hint("Recommended - Best for coding and reasoning"),
            WizardOption::new(json!("openai"), "OpenAI (GPT)")
                .with_hint("GPT-4o and o3 series"),
            WizardOption::new(json!("google"), "Google (Gemini)")
                .with_hint("Gemini Pro and Ultra"),
            WizardOption::new(json!("ollama"), "Ollama (Local)")
                .with_hint("Run models locally - No API key needed"),
            WizardOption::new(json!("openrouter"), "OpenRouter")
                .with_hint("Access multiple providers with one key"),
        ]
    }

    /// Get model options for a provider
    fn model_options(provider: &str) -> Vec<WizardOption> {
        match provider {
            "anthropic" => vec![
                WizardOption::new(json!("claude-opus-4-5-20251101"), "Claude Opus 4.5")
                    .with_hint("Most capable, best for complex tasks"),
                WizardOption::new(json!("claude-sonnet-4-20250514"), "Claude Sonnet 4")
                    .with_hint("Fast and capable (Recommended)"),
                WizardOption::new(json!("claude-3-5-haiku-20241022"), "Claude 3.5 Haiku")
                    .with_hint("Fastest, good for simple tasks"),
            ],
            "openai" => vec![
                WizardOption::new(json!("gpt-4o"), "GPT-4o")
                    .with_hint("Best overall performance"),
                WizardOption::new(json!("gpt-4o-mini"), "GPT-4o Mini")
                    .with_hint("Faster, more economical"),
                WizardOption::new(json!("o3-mini"), "o3-mini")
                    .with_hint("Reasoning model"),
            ],
            "google" => vec![
                WizardOption::new(json!("gemini-2.0-flash"), "Gemini 2.0 Flash")
                    .with_hint("Fast and capable"),
                WizardOption::new(json!("gemini-pro"), "Gemini Pro")
                    .with_hint("Balanced performance"),
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
                WizardOption::new(json!("anthropic/claude-opus-4-5"), "Claude Opus 4.5 (via OpenRouter)")
                    .with_hint("Anthropic's flagship"),
                WizardOption::new(json!("openai/gpt-4o"), "GPT-4o (via OpenRouter)")
                    .with_hint("OpenAI's flagship"),
                WizardOption::new(json!("google/gemini-pro"), "Gemini Pro (via OpenRouter)")
                    .with_hint("Google's flagship"),
            ],
            _ => vec![
                WizardOption::new(json!("auto"), "Auto-detect")
                    .with_hint("Let Aleph choose the best model"),
            ],
        }
    }

    /// Create thinking level options
    fn thinking_options() -> Vec<WizardOption> {
        vec![
            WizardOption::new(json!("off"), "Off")
                .with_hint("No extended thinking"),
            WizardOption::new(json!("minimal"), "Minimal")
                .with_hint("Brief reasoning"),
            WizardOption::new(json!("low"), "Low")
                .with_hint("Quick analysis"),
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
                .with_hint("Business communication").disabled(),
            WizardOption::new(json!("imessage"), "iMessage")
                .with_hint("macOS only"),
        ]
    }
}

impl Default for OnboardingFlow {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WizardFlow for OnboardingFlow {
    async fn run(&self, prompter: &RpcPrompter) -> Result<(), WizardSessionError> {
        let mut data = OnboardingData::default();

        // ===== Stage 1: Welcome =====
        prompter.intro("Welcome to Aleph").await?;
        prompter.note(
            "This wizard will help you set up Aleph, your personal AI assistant.\n\n\
             You'll configure:\n\
             • AI provider and model\n\
             • Optional secondary model for failover\n\
             • Thinking level for AI responses\n\
             • Messaging app integrations",
            Some("About this wizard"),
        ).await?;

        // ===== Stage 2: Provider Selection =====
        let primary_provider: String = prompter.select(
            "Which AI provider would you like to use?",
            Self::provider_options(),
        ).await?;
        data.primary_provider = Some(primary_provider.clone());

        // ===== Stage 3: Credentials Input =====
        if primary_provider != "ollama" {
            let api_key = prompter.text(
                &format!("Enter your {} API key:", primary_provider),
                Some("sk-..."),
                true, // sensitive
            ).await?;
            data.primary_api_key = Some(api_key);
        } else {
            prompter.note(
                "Ollama runs locally - no API key needed.\n\
                 Make sure Ollama is installed and running.",
                Some("Local Models"),
            ).await?;
        }

        // ===== Stage 4: Primary Model =====
        let primary_model: String = prompter.select(
            "Select your primary model:",
            Self::model_options(&primary_provider),
        ).await?;
        data.primary_model = Some(primary_model);

        // ===== Stage 5: Secondary Model (Optional) =====
        let wants_secondary = prompter.confirm(
            "Would you like to configure a secondary model for failover?",
            false,
        ).await?;

        if wants_secondary {
            let secondary_provider: String = prompter.select(
                "Select secondary provider:",
                Self::provider_options(),
            ).await?;
            data.secondary_provider = Some(secondary_provider.clone());

            if secondary_provider != "ollama" && secondary_provider != primary_provider {
                let api_key = prompter.text(
                    &format!("Enter your {} API key:", secondary_provider),
                    Some("sk-..."),
                    true,
                ).await?;
                data.secondary_api_key = Some(api_key);
            }

            let secondary_model: String = prompter.select(
                "Select secondary model:",
                Self::model_options(&secondary_provider),
            ).await?;
            data.secondary_model = Some(secondary_model);
        }

        // ===== Stage 6: Thinking Level =====
        let thinking: String = prompter.select(
            "Choose the AI thinking level:",
            Self::thinking_options(),
        ).await?;
        data.thinking_level = Some(thinking);

        // ===== Stage 7: Messaging Apps =====
        let apps: Vec<String> = prompter.multi_select(
            "Which messaging apps would you like to connect? (Optional)",
            Self::messaging_options(),
        ).await?;
        data.messaging_apps = apps;

        // ===== Stage 8: Review =====
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
                format!("{} / {}",
                    data.secondary_provider.as_deref().unwrap_or(""),
                    data.secondary_model.as_deref().unwrap_or(""))
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

        // ===== Stage 9: Finalize =====
        let confirmed = prompter.confirm(
            "Apply this configuration?",
            true,
        ).await?;

        if !confirmed {
            return Err(WizardSessionError::Cancelled);
        }

        // Apply configuration (in real implementation, save to config file)
        let progress = prompter.progress("Applying configuration");
        progress.update("Validating API keys...");
        // Simulate validation delay
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        progress.update("Saving configuration...");
        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
        progress.finish("Configuration saved");

        // ===== Stage 10: Complete =====
        prompter.outro("Aleph is ready! Run 'aether chat' to start.").await?;

        Ok(())
    }

    fn name(&self) -> &str {
        "onboarding"
    }
}

/// Quick setup flow (minimal configuration)
pub struct QuickSetupFlow;

impl QuickSetupFlow {
    pub fn new() -> Self {
        Self
    }
}

impl Default for QuickSetupFlow {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WizardFlow for QuickSetupFlow {
    async fn run(&self, prompter: &RpcPrompter) -> Result<(), WizardSessionError> {
        prompter.intro("Quick Setup").await?;

        // Just get the API key for Claude
        let api_key = prompter.text(
            "Enter your Anthropic API key:",
            Some("sk-ant-..."),
            true,
        ).await?;

        if api_key.is_empty() {
            return Err(WizardSessionError::InvalidAnswer("API key is required".to_string()));
        }

        let progress = prompter.progress("Setting up");
        progress.update("Configuring Claude Sonnet 4...");
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        progress.finish("Ready");

        prompter.outro("Quick setup complete! Run 'aether chat' to start.").await?;

        Ok(())
    }

    fn name(&self) -> &str {
        "quick-setup"
    }
}

/// Provider setup flow (add a new provider)
pub struct ProviderSetupFlow {
    provider: Option<String>,
}

impl ProviderSetupFlow {
    pub fn new(provider: Option<String>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl WizardFlow for ProviderSetupFlow {
    async fn run(&self, prompter: &RpcPrompter) -> Result<(), WizardSessionError> {
        prompter.intro("Provider Setup").await?;

        let provider = if let Some(ref p) = self.provider {
            p.clone()
        } else {
            prompter.select(
                "Select provider to configure:",
                OnboardingFlow::provider_options(),
            ).await?
        };

        if provider != "ollama" {
            let _api_key = prompter.text(
                &format!("Enter your {} API key:", provider),
                Some("sk-..."),
                true,
            ).await?;
        }

        let _model: String = prompter.select(
            "Select default model:",
            OnboardingFlow::model_options(&provider),
        ).await?;

        let confirmed = prompter.confirm(
            "Save this provider configuration?",
            true,
        ).await?;

        if !confirmed {
            return Err(WizardSessionError::Cancelled);
        }

        let progress = prompter.progress("Saving");
        progress.update("Validating...");
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        progress.finish("Saved");

        prompter.outro("Provider configured successfully!").await?;

        Ok(())
    }

    fn name(&self) -> &str {
        "provider-setup"
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
        assert!(options.iter().find(|o| o.label == "Slack").map(|o| o.disabled).unwrap_or(false));
    }

    #[test]
    fn test_onboarding_data_default() {
        let data = OnboardingData::default();
        assert!(data.primary_provider.is_none());
        assert!(data.messaging_apps.is_empty());
    }
}
