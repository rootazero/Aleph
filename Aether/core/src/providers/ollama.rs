/// Ollama local LLM client implementation
///
/// Implements the `AiProvider` trait for locally-hosted Ollama models.
/// Executes the `ollama run` command to generate responses.
///
/// # Configuration
///
/// Required fields:
/// - `model`: Model name (e.g., "llama3.2", "codellama", "mistral")
///
/// Optional fields:
/// - `timeout_seconds`: Command execution timeout (defaults to 30)
///
/// # Prerequisites
///
/// Ollama must be installed and available in PATH:
/// - macOS/Linux: `curl -fsSL https://ollama.ai/install.sh | sh`
/// - Manual: https://ollama.ai/download
///
/// Models must be pulled before use:
/// ```bash
/// ollama pull llama3.2
/// ```
///
/// # Design Notes
///
/// Unlike cloud providers (OpenAI, Claude), Ollama runs locally:
/// - No API key required
/// - No network calls (assuming local Ollama server)
/// - Uses command-line execution via `tokio::process::Command`
/// - Output may contain ANSI escape codes that need stripping
///
/// # Example
///
/// ```rust,no_run
/// use aethecore::config::ProviderConfig;
/// use aethecore::providers::ollama::OllamaProvider;
/// use aethecore::providers::AiProvider;
///
/// # async fn example() -> aethecore::error::Result<()> {
/// let config = ProviderConfig {
///     api_key: None, // Not needed for Ollama
///     model: "llama3.2".to_string(),
///     base_url: None,
///     color: "#0000ff".to_string(),
///     timeout_seconds: 60,
///     max_tokens: None,
///     temperature: None,
/// };
///
/// let provider = OllamaProvider::new(config)?;
/// let response = provider.process("Hello!", Some("You are helpful")).await?;
/// println!("Response: {}", response);
/// # Ok(())
/// # }
/// ```

use crate::config::ProviderConfig;
use crate::error::{AetherError, Result};
use crate::providers::AiProvider;
use async_trait::async_trait;
use regex::Regex;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

/// Ollama local provider
pub struct OllamaProvider {
    /// Model name (e.g., "llama3.2")
    model: String,
    /// Command execution timeout
    timeout: Duration,
    /// Provider brand color
    color: String,
}

impl OllamaProvider {
    /// Create new Ollama provider
    ///
    /// # Arguments
    ///
    /// * `config` - Provider configuration with model name
    ///
    /// # Returns
    ///
    /// * `Ok(OllamaProvider)` - Successfully initialized provider
    /// * `Err(AetherError)` - Configuration validation failed
    ///
    /// # Errors
    ///
    /// Returns `InvalidConfig` if:
    /// - Model name is empty
    /// - Timeout is zero
    pub fn new(config: ProviderConfig) -> Result<Self> {
        if config.model.is_empty() {
            return Err(AetherError::InvalidConfig(
                "Model name cannot be empty".to_string(),
            ));
        }

        if config.timeout_seconds == 0 {
            return Err(AetherError::InvalidConfig(
                "Timeout must be greater than zero".to_string(),
            ));
        }

        Ok(Self {
            model: config.model,
            timeout: Duration::from_secs(config.timeout_seconds),
            color: config.color,
        })
    }

    /// Format prompt combining system prompt and user input
    ///
    /// If a system prompt is provided, prepend it to the user input.
    /// This mimics the behavior of chat APIs where system prompts guide model behavior.
    fn format_prompt(&self, input: &str, system_prompt: Option<&str>) -> String {
        match system_prompt {
            Some(sys) => format!("{}\n\n{}", sys, input),
            None => input.to_string(),
        }
    }

    /// Strip ANSI escape codes from output
    ///
    /// Ollama may include color codes and formatting in terminal output.
    /// This function removes all ANSI sequences to get clean text.
    fn strip_ansi_codes(&self, text: &str) -> String {
        // Regex pattern for ANSI escape codes
        // Matches ESC [ ... m sequences
        let ansi_pattern = Regex::new(r"\x1b\[[0-9;]*m").unwrap();
        ansi_pattern.replace_all(text, "").to_string()
    }

    /// Clean and format output text
    ///
    /// - Strip ANSI escape codes
    /// - Trim leading/trailing whitespace
    /// - Preserve internal line breaks
    fn clean_output(&self, output: &str) -> String {
        let stripped = self.strip_ansi_codes(output);
        stripped.trim().to_string()
    }
}

#[async_trait]
impl AiProvider for OllamaProvider {
    async fn process(&self, input: &str, system_prompt: Option<&str>) -> Result<String> {
        // Format prompt
        let prompt = self.format_prompt(input, system_prompt);

        // Build command: ollama run <model> <prompt>
        let mut cmd = Command::new("ollama");
        cmd.arg("run")
            .arg(&self.model)
            .arg(&prompt)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        // Execute with timeout
        let output_result = timeout(self.timeout, cmd.output()).await;

        // Handle timeout
        let output = match output_result {
            Ok(result) => result.map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    AetherError::ProviderError(
                        "Ollama command not found. Please install Ollama from https://ollama.ai"
                            .to_string(),
                    )
                } else {
                    AetherError::ProviderError(format!("Failed to execute Ollama: {}", e))
                }
            })?,
            Err(_) => return Err(AetherError::Timeout),
        };

        // Check exit status
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);

            // Check for specific error patterns
            if stderr.contains("model") && stderr.contains("not found") {
                return Err(AetherError::ProviderError(format!(
                    "Ollama model '{}' not found. Run: ollama pull {}",
                    self.model, self.model
                )));
            }

            return Err(AetherError::ProviderError(format!(
                "Ollama command failed (exit {}): {}",
                output.status.code().unwrap_or(-1),
                stderr
            )));
        }

        // Parse stdout
        let raw_output = String::from_utf8(output.stdout).map_err(|e| {
            AetherError::ProviderError(format!("Ollama output is not valid UTF-8: {}", e))
        })?;

        // Clean output
        let cleaned = self.clean_output(&raw_output);

        if cleaned.is_empty() {
            return Err(AetherError::ProviderError(
                "Ollama returned empty response".to_string(),
            ));
        }

        Ok(cleaned)
    }

    fn name(&self) -> &str {
        "ollama"
    }

    fn color(&self) -> &str {
        &self.color
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> ProviderConfig {
        ProviderConfig {
            provider_type: None,
            api_key: None, // Not needed for Ollama
            model: "llama3.2".to_string(),
            base_url: None,
            color: "#0000ff".to_string(),
            timeout_seconds: 60,
            max_tokens: None,
            temperature: None,
        }
    }

    #[test]
    fn test_new_provider_success() {
        let config = create_test_config();
        let provider = OllamaProvider::new(config);
        assert!(provider.is_ok());
    }

    #[test]
    fn test_new_provider_empty_model() {
        let mut config = create_test_config();
        config.model = "".to_string();
        let result = OllamaProvider::new(config);
        assert!(matches!(result, Err(AetherError::InvalidConfig(_))));
    }

    #[test]
    fn test_new_provider_zero_timeout() {
        let mut config = create_test_config();
        config.timeout_seconds = 0;
        let result = OllamaProvider::new(config);
        assert!(matches!(result, Err(AetherError::InvalidConfig(_))));
    }

    #[test]
    fn test_format_prompt_without_system() {
        let config = create_test_config();
        let provider = OllamaProvider::new(config).unwrap();

        let prompt = provider.format_prompt("Hello", None);
        assert_eq!(prompt, "Hello");
    }

    #[test]
    fn test_format_prompt_with_system() {
        let config = create_test_config();
        let provider = OllamaProvider::new(config).unwrap();

        let prompt = provider.format_prompt("Hello", Some("You are a helpful assistant"));
        assert_eq!(prompt, "You are a helpful assistant\n\nHello");
    }

    #[test]
    fn test_strip_ansi_codes() {
        let config = create_test_config();
        let provider = OllamaProvider::new(config).unwrap();

        // Test with ANSI color codes
        let text_with_ansi = "\x1b[32mGreen text\x1b[0m normal text \x1b[1;31mRed bold\x1b[0m";
        let stripped = provider.strip_ansi_codes(text_with_ansi);
        assert_eq!(stripped, "Green text normal text Red bold");
    }

    #[test]
    fn test_strip_ansi_codes_no_ansi() {
        let config = create_test_config();
        let provider = OllamaProvider::new(config).unwrap();

        let plain_text = "Hello world";
        let result = provider.strip_ansi_codes(plain_text);
        assert_eq!(result, plain_text);
    }

    #[test]
    fn test_clean_output() {
        let config = create_test_config();
        let provider = OllamaProvider::new(config).unwrap();

        // Test with ANSI codes and whitespace
        let raw = "  \n\x1b[32mHello\x1b[0m world\n  ";
        let cleaned = provider.clean_output(raw);
        assert_eq!(cleaned, "Hello world");
    }

    #[test]
    fn test_clean_output_multiline() {
        let config = create_test_config();
        let provider = OllamaProvider::new(config).unwrap();

        let raw = "Line 1\nLine 2\nLine 3";
        let cleaned = provider.clean_output(raw);
        assert_eq!(cleaned, "Line 1\nLine 2\nLine 3");
    }

    #[test]
    fn test_provider_metadata() {
        let config = create_test_config();
        let provider = OllamaProvider::new(config).unwrap();

        assert_eq!(provider.name(), "ollama");
        assert_eq!(provider.color(), "#0000ff");
    }

    #[test]
    fn test_timeout_value() {
        let config = create_test_config();
        let provider = OllamaProvider::new(config).unwrap();

        assert_eq!(provider.timeout, Duration::from_secs(60));
    }

    // Note: Integration tests requiring actual Ollama installation
    // should be in tests/ directory and gated behind a feature flag
    // Example:
    // #[tokio::test]
    // #[cfg(feature = "integration-tests")]
    // async fn test_ollama_integration() {
    //     // This would require ollama to be installed and running
    //     // with llama3.2 model pulled
    // }
}
