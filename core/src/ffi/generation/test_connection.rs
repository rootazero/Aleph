//! Provider connection testing for generation providers
//!
//! This module contains the test_connection method for validating provider configuration.

use crate::ffi::AetherCore;
use crate::generation::{GenerationRequest, GenerationType};
use tracing::info;

impl AetherCore {
    /// Test a generation provider connection with temporary configuration
    ///
    /// This method tests a generation provider without persisting the configuration.
    /// It sends a minimal test request to verify the API is reachable and responsive.
    ///
    /// # Arguments
    ///
    /// * `provider_type` - Provider type: "openai_compat", "openai", "stability", etc.
    /// * `api_key` - API key for authentication
    /// * `base_url` - Base URL for the API (optional, e.g., "https://api.example.com/v1")
    /// * `model` - Model name (optional, e.g., "dall-e-3")
    ///
    /// # Returns
    ///
    /// Test result with success status and message
    pub fn test_generation_provider_connection(
        &self,
        provider_type: String,
        api_key: String,
        base_url: Option<String>,
        model: Option<String>,
    ) -> crate::config::TestConnectionResult {
        use crate::config::GenerationProviderConfig;
        use crate::generation::providers::create_provider;

        info!(
            provider_type = %provider_type,
            base_url = ?base_url,
            model = ?model,
            "Testing generation provider connection"
        );

        // Build provider config
        let provider_config = GenerationProviderConfig {
            provider_type: provider_type.clone(),
            api_key: Some(api_key),
            base_url,
            model,
            enabled: true,
            color: "#808080".to_string(),
            capabilities: vec![GenerationType::Image],
            timeout_seconds: 120,
            defaults: Default::default(),
            models: Default::default(),
        };

        // Create provider instance
        let provider = match create_provider("test-connection", &provider_config) {
            Ok(p) => p,
            Err(e) => {
                return crate::config::TestConnectionResult {
                    success: false,
                    message: format!("Failed to create provider: {}", e),
                };
            }
        };

        // Test by generating a simple image with minimal prompt
        // Don't specify size - let the API use its default (more compatible with different models)
        let test_request = GenerationRequest::new(GenerationType::Image, "a white dot")
            .with_params(crate::generation::GenerationParams::builder().n(1).build());

        let result = self.runtime.block_on(async {
            // Use tokio timeout for safety (120 seconds for image generation)
            match tokio::time::timeout(
                std::time::Duration::from_secs(120),
                provider.generate(test_request),
            )
            .await
            {
                Ok(Ok(output)) => {
                    let data_type = if output.data.is_url() {
                        "URL"
                    } else if output.data.is_bytes() {
                        "bytes"
                    } else {
                        "file"
                    };
                    Ok(format!(
                        "Image generated successfully ({} returned)",
                        data_type
                    ))
                }
                Ok(Err(e)) => Err(format!("{}", e)),
                Err(_) => Err("Connection timed out after 120 seconds".to_string()),
            }
        });

        match result {
            Ok(msg) => crate::config::TestConnectionResult {
                success: true,
                message: format!("✓ {}", msg),
            },
            Err(err_msg) => crate::config::TestConnectionResult {
                success: false,
                message: err_msg,
            },
        }
    }
}
