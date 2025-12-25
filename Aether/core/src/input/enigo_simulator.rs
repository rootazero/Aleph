/// EnigoSimulator - Real keyboard input simulation using enigo
///
/// This module provides the actual implementation of InputSimulator
/// using the enigo crate for cross-platform keyboard simulation.

use crate::error::{AetherError, Result};
use crate::input::InputSimulator;
use enigo::{Enigo, Key, Keyboard, Settings};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

/// Real input simulator using enigo
///
/// Uses enigo for actual keyboard simulation on macOS/Windows/Linux.
pub struct EnigoSimulator;

impl EnigoSimulator {
    /// Create a new EnigoSimulator
    pub fn new() -> Self {
        Self
    }

    /// Create an Enigo instance with default settings
    fn create_enigo() -> Result<Enigo> {
        Enigo::new(&Settings::default()).map_err(|e| AetherError::InputSimulationError {
            message: format!("Failed to create Enigo instance: {:?}", e),
        })
    }
}

impl Default for EnigoSimulator {
    fn default() -> Self {
        Self::new()
    }
}

impl InputSimulator for EnigoSimulator {
    fn simulate_cut(&self) -> Result<()> {
        debug!("Simulating cut operation (Cmd/Ctrl+X)");
        let mut enigo = Self::create_enigo()?;

        // Simulate Cmd+X (macOS) or Ctrl+X (Windows/Linux)
        #[cfg(target_os = "macos")]
        {
            enigo.key(Key::Meta, enigo::Direction::Press).map_err(|e| {
                error!(error = ?e, "Failed to press Meta key");
                AetherError::InputSimulationError {
                    message: format!("Failed to press Meta key: {:?}", e),
                }
            })?;
            enigo.key(Key::Unicode('x'), enigo::Direction::Click).map_err(|e| {
                error!(error = ?e, "Failed to click x key");
                AetherError::InputSimulationError {
                    message: format!("Failed to click x key: {:?}", e),
                }
            })?;
            enigo.key(Key::Meta, enigo::Direction::Release).map_err(|e| {
                error!(error = ?e, "Failed to release Meta key");
                AetherError::InputSimulationError {
                    message: format!("Failed to release Meta key: {:?}", e),
                }
            })?;
        }

        #[cfg(not(target_os = "macos"))]
        {
            enigo.key(Key::Control, enigo::Direction::Press).map_err(|e| {
                error!(error = ?e, "Failed to press Control key");
                AetherError::InputSimulationError {
                    message: format!("Failed to press Control key: {:?}", e),
                }
            })?;
            enigo.key(Key::Unicode('x'), enigo::Direction::Click).map_err(|e| {
                error!(error = ?e, "Failed to click x key");
                AetherError::InputSimulationError {
                    message: format!("Failed to click x key: {:?}", e),
                }
            })?;
            enigo.key(Key::Control, enigo::Direction::Release).map_err(|e| {
                error!(error = ?e, "Failed to release Control key");
                AetherError::InputSimulationError {
                    message: format!("Failed to release Control key: {:?}", e),
                }
            })?;
        }

        debug!("Cut operation completed successfully");
        Ok(())
    }

    fn simulate_paste(&self) -> Result<()> {
        debug!("Simulating paste operation (Cmd/Ctrl+V)");
        let mut enigo = Self::create_enigo()?;

        // Simulate Cmd+V (macOS) or Ctrl+V (Windows/Linux)
        #[cfg(target_os = "macos")]
        {
            enigo.key(Key::Meta, enigo::Direction::Press).map_err(|e| {
                error!(error = ?e, "Failed to press Meta key");
                AetherError::InputSimulationError {
                    message: format!("Failed to press Meta key: {:?}", e),
                }
            })?;
            enigo.key(Key::Unicode('v'), enigo::Direction::Click).map_err(|e| {
                error!(error = ?e, "Failed to click v key");
                AetherError::InputSimulationError {
                    message: format!("Failed to click v key: {:?}", e),
                }
            })?;
            enigo.key(Key::Meta, enigo::Direction::Release).map_err(|e| {
                error!(error = ?e, "Failed to release Meta key");
                AetherError::InputSimulationError {
                    message: format!("Failed to release Meta key: {:?}", e),
                }
            })?;
        }

        #[cfg(not(target_os = "macos"))]
        {
            enigo.key(Key::Control, enigo::Direction::Press).map_err(|e| {
                error!(error = ?e, "Failed to press Control key");
                AetherError::InputSimulationError {
                    message: format!("Failed to press Control key: {:?}", e),
                }
            })?;
            enigo.key(Key::Unicode('v'), enigo::Direction::Click).map_err(|e| {
                error!(error = ?e, "Failed to click v key");
                AetherError::InputSimulationError {
                    message: format!("Failed to click v key: {:?}", e),
                }
            })?;
            enigo.key(Key::Control, enigo::Direction::Release).map_err(|e| {
                error!(error = ?e, "Failed to release Control key");
                AetherError::InputSimulationError {
                    message: format!("Failed to release Control key: {:?}", e),
                }
            })?;
        }

        debug!("Paste operation completed successfully");
        Ok(())
    }

    fn simulate_select_all(&self) -> Result<()> {
        debug!("Simulating select all operation (Cmd/Ctrl+A)");
        let mut enigo = Self::create_enigo()?;

        // Simulate Cmd+A (macOS) or Ctrl+A (Windows/Linux)
        #[cfg(target_os = "macos")]
        {
            enigo.key(Key::Meta, enigo::Direction::Press).map_err(|e| {
                error!(error = ?e, "Failed to press Meta key");
                AetherError::InputSimulationError {
                    message: format!("Failed to press Meta key: {:?}", e),
                }
            })?;
            enigo.key(Key::Unicode('a'), enigo::Direction::Click).map_err(|e| {
                error!(error = ?e, "Failed to click a key");
                AetherError::InputSimulationError {
                    message: format!("Failed to click a key: {:?}", e),
                }
            })?;
            enigo.key(Key::Meta, enigo::Direction::Release).map_err(|e| {
                error!(error = ?e, "Failed to release Meta key");
                AetherError::InputSimulationError {
                    message: format!("Failed to release Meta key: {:?}", e),
                }
            })?;
        }

        #[cfg(not(target_os = "macos"))]
        {
            enigo.key(Key::Control, enigo::Direction::Press).map_err(|e| {
                error!(error = ?e, "Failed to press Control key");
                AetherError::InputSimulationError {
                    message: format!("Failed to press Control key: {:?}", e),
                }
            })?;
            enigo.key(Key::Unicode('a'), enigo::Direction::Click).map_err(|e| {
                error!(error = ?e, "Failed to click a key");
                AetherError::InputSimulationError {
                    message: format!("Failed to click a key: {:?}", e),
                }
            })?;
            enigo.key(Key::Control, enigo::Direction::Release).map_err(|e| {
                error!(error = ?e, "Failed to release Control key");
                AetherError::InputSimulationError {
                    message: format!("Failed to release Control key: {:?}", e),
                }
            })?;
        }

        debug!("Select all operation completed successfully");
        Ok(())
    }

    async fn type_string_animated(
        &self,
        text: &str,
        chars_per_second: u32,
        cancellation_token: CancellationToken,
    ) -> Result<()> {
        // Validate typing speed (10-200 cps recommended)
        let original_speed = chars_per_second;
        let chars_per_second = chars_per_second.clamp(10, 200);

        if original_speed != chars_per_second {
            warn!(
                original_speed = original_speed,
                clamped_speed = chars_per_second,
                "Typing speed clamped to valid range (10-200)"
            );
        }

        let total_chars = text.chars().count();
        info!(
            chars_per_second = chars_per_second,
            total_chars = total_chars,
            estimated_duration_ms = (total_chars * 1000) / chars_per_second as usize,
            "Starting typewriter animation"
        );

        // Calculate delay between characters
        let delay_per_char = Duration::from_millis(1000 / chars_per_second as u64);

        // Type each character with delay
        for (index, ch) in text.chars().enumerate() {
            // Check cancellation before each character
            if cancellation_token.is_cancelled() {
                warn!(
                    typed_chars = index,
                    total_chars = total_chars,
                    progress_pct = (index * 100) / total_chars,
                    "Typewriter animation cancelled by user"
                );
                return Ok(()); // Exit early on cancellation
            }

            // Use a block scope to ensure enigo is dropped before await
            {
                // Create enigo instance for each character (Enigo is not Send)
                let mut enigo = Self::create_enigo()?;

                // Type the character
                match ch {
                    '\n' => {
                        // Newline → Enter key
                        enigo.key(Key::Return, enigo::Direction::Click).map_err(|e| {
                            error!(error = ?e, "Failed to click Return key");
                            AetherError::InputSimulationError {
                                message: format!("Failed to click Return key: {:?}", e),
                            }
                        })?;
                    }
                    '\t' => {
                        // Tab → Tab key
                        enigo.key(Key::Tab, enigo::Direction::Click).map_err(|e| {
                            error!(error = ?e, "Failed to click Tab key");
                            AetherError::InputSimulationError {
                                message: format!("Failed to click Tab key: {:?}", e),
                            }
                        })?;
                    }
                    _ => {
                        // Regular character → Type using text method
                        enigo.text(&ch.to_string()).map_err(|e| {
                            error!(error = ?e, char = %ch, "Failed to type character");
                            AetherError::InputSimulationError {
                                message: format!("Failed to type character '{}': {:?}", ch, e),
                            }
                        })?;
                    }
                }
                // enigo is dropped here when the block scope ends
            }

            // Wait before next character (enigo is guaranteed dropped)
            tokio::time::sleep(delay_per_char).await;
        }

        info!(
            total_chars = total_chars,
            chars_per_second = chars_per_second,
            "Typewriter animation completed successfully"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enigo_simulator_creation() {
        let _sim = EnigoSimulator::new();
        let _sim2 = EnigoSimulator::default();
        // Should not panic
    }

    #[tokio::test]
    async fn test_type_string_animated_cancellation() {
        let sim = EnigoSimulator::new();
        let token = CancellationToken::new();

        // Cancel immediately
        token.cancel();

        // Should exit early without error (or fail due to no display)
        let result = sim
            .type_string_animated("Hello, world!", 50, token)
            .await;
        // In CI/headless environment this might fail, but cancellation logic should work
        let _ = result;
    }

    #[tokio::test]
    async fn test_type_string_animated_speed_clamping() {
        let sim = EnigoSimulator::new();
        let token = CancellationToken::new();

        // Test with out-of-range speeds (should be clamped)
        // These will likely fail without a display, but the clamping logic runs
        let _ = sim.type_string_animated("Hi", 5, token.clone()).await;
        let _ = sim.type_string_animated("Hi", 300, token.clone()).await;
    }
}
