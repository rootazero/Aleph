//! FFI interface for typo correction
//!
//! Provides the FFI entry point for the quick typo correction feature.
//! This module exposes a simple async function that can be called from Swift.

use super::AetherCore;
use crate::providers::create_provider;
use crate::typo_correction::{CorrectionResult, TypoCorrector};
use tracing::{debug, error, info, warn};

/// FFI result type for typo correction
#[derive(Debug, Clone)]
pub enum TypoCorrectionResult {
    /// Correction succeeded
    Success {
        /// The corrected text
        corrected_text: String,
        /// Whether any changes were made
        has_changes: bool,
    },
    /// Correction failed
    Error {
        /// Error message
        message: String,
    },
}

impl From<CorrectionResult> for TypoCorrectionResult {
    fn from(result: CorrectionResult) -> Self {
        TypoCorrectionResult::Success {
            corrected_text: result.corrected_text,
            has_changes: result.has_changes,
        }
    }
}

impl AetherCore {
    /// Correct typos in the given text
    ///
    /// This is the FFI entry point for typo correction. It:
    /// 1. Checks if typo correction is enabled
    /// 2. Creates a provider based on configuration
    /// 3. Calls the TypoCorrector
    /// 4. Returns the result
    ///
    /// # Arguments
    ///
    /// * `text` - The text to correct
    ///
    /// # Returns
    ///
    /// * `TypoCorrectionResult::Success` - Correction succeeded
    /// * `TypoCorrectionResult::Error` - Correction failed
    pub async fn correct_typo(&self, text: String) -> TypoCorrectionResult {
        debug!("FFI correct_typo called with {} chars", text.len());

        // Get typo correction config
        let typo_config = {
            let config = self.lock_config();
            config.typo_correction.clone()
        };

        // Check if enabled
        if !typo_config.enabled {
            warn!("Typo correction is disabled in config");
            return TypoCorrectionResult::Error {
                message: "Typo correction is disabled".to_string(),
            };
        }

        // Get provider name
        let provider_name = match &typo_config.provider {
            Some(name) => name.clone(),
            None => {
                error!("No provider configured for typo correction");
                return TypoCorrectionResult::Error {
                    message: "No provider configured for typo correction".to_string(),
                };
            }
        };

        // Get provider config and create provider
        let provider = {
            let config = self.lock_config();
            match config.providers.get(&provider_name) {
                Some(provider_config) => {
                    // Clone the provider config and optionally override the model
                    let mut config_to_use = provider_config.clone();
                    if let Some(ref model_override) = typo_config.model {
                        config_to_use.model = model_override.clone();
                    }

                    match create_provider(&provider_name, config_to_use) {
                        Ok(provider) => provider,
                        Err(e) => {
                            error!("Failed to create provider '{}': {}", provider_name, e);
                            return TypoCorrectionResult::Error {
                                message: format!("Failed to create provider: {}", e),
                            };
                        }
                    }
                }
                None => {
                    error!("Provider '{}' not found in config", provider_name);
                    return TypoCorrectionResult::Error {
                        message: format!("Provider '{}' not found in config", provider_name),
                    };
                }
            }
        };

        info!(
            provider = %provider_name,
            model = typo_config.model.as_deref().unwrap_or("(default)"),
            "Performing typo correction"
        );

        // Create corrector and perform correction
        let corrector = TypoCorrector::new(provider, typo_config);

        match corrector.correct(&text).await {
            Ok(result) => {
                if result.has_changes {
                    info!("Text corrected successfully");
                } else {
                    debug!("No corrections needed");
                }
                result.into()
            }
            Err(e) => {
                error!("Typo correction failed: {}", e);
                TypoCorrectionResult::Error {
                    message: e.to_string(),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_typo_correction_result_from() {
        let result = CorrectionResult {
            corrected_text: "corrected".to_string(),
            has_changes: true,
        };

        let ffi_result: TypoCorrectionResult = result.into();
        match ffi_result {
            TypoCorrectionResult::Success {
                corrected_text,
                has_changes,
            } => {
                assert_eq!(corrected_text, "corrected");
                assert!(has_changes);
            }
            _ => panic!("Expected Success variant"),
        }
    }
}
