//! Screen Capture Tool
//!
//! Provides screen and window capture via the AgentTool trait.

use async_trait::async_trait;
use base64::{engine::general_purpose, Engine as _};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

use crate::error::{AetherError, Result};
use crate::tools::{AgentTool, ToolCategory, ToolDefinition, ToolResult};

/// Screen capture configuration
#[derive(Debug, Clone)]
pub struct ScreenConfig {
    /// Whether screen capture is enabled
    pub enabled: bool,
    /// Maximum dimension for captured images (pixels)
    /// Images larger than this will be resized
    pub max_dimension: u32,
    /// JPEG quality (1-100)
    pub jpeg_quality: u8,
}

impl Default for ScreenConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_dimension: 1920,
            jpeg_quality: 85,
        }
    }
}

impl ScreenConfig {
    /// Create a disabled configuration
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Default::default()
        }
    }

    /// Set max dimension
    pub fn with_max_dimension(mut self, max: u32) -> Self {
        self.max_dimension = max;
        self
    }
}

/// Screen tools context
///
/// Provides shared access to screen capture configuration.
#[derive(Clone)]
pub struct ScreenContext {
    /// Configuration
    pub config: Arc<ScreenConfig>,
}

impl ScreenContext {
    /// Create a new context
    pub fn new(config: ScreenConfig) -> Self {
        Self {
            config: Arc::new(config),
        }
    }

    /// Check if screen capture is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Resize image if it exceeds max dimension
    fn resize_if_needed(&self, img: xcap::image::RgbaImage) -> xcap::image::RgbaImage {
        let (w, h) = img.dimensions();
        let max = self.config.max_dimension;

        if w <= max && h <= max {
            return img;
        }

        let scale = if w > h {
            max as f32 / w as f32
        } else {
            max as f32 / h as f32
        };

        let new_w = (w as f32 * scale) as u32;
        let new_h = (h as f32 * scale) as u32;

        xcap::image::imageops::resize(
            &img,
            new_w,
            new_h,
            xcap::image::imageops::FilterType::Lanczos3,
        )
    }

    /// Encode image to JPEG and base64
    fn encode_image(&self, img: xcap::image::RgbaImage) -> Result<(String, u32, u32)> {
        let (w, h) = img.dimensions();

        // Convert RGBA to RGB for JPEG encoding
        let rgb_img: xcap::image::RgbImage =
            xcap::image::DynamicImage::ImageRgba8(img).to_rgb8();

        let mut buffer = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut buffer);

        rgb_img
            .write_to(&mut cursor, xcap::image::ImageFormat::Jpeg)
            .map_err(|e| AetherError::other(format!("Failed to encode image: {}", e)))?;

        let base64_data = general_purpose::STANDARD.encode(&buffer);

        Ok((base64_data, w, h))
    }
}

impl Default for ScreenContext {
    fn default() -> Self {
        Self::new(ScreenConfig::default())
    }
}

/// Parameters for screen_capture tool
#[derive(Debug, Deserialize)]
struct ScreenCaptureParams {
    /// Target to capture: "screen", "window", "monitors", "windows"
    #[serde(default = "default_target")]
    target: String,
    /// Monitor index for screen capture
    #[serde(default)]
    monitor_index: Option<usize>,
    /// Window title substring to match
    #[serde(default)]
    window_title: Option<String>,
}

fn default_target() -> String {
    "screen".to_string()
}

/// Screen capture tool
///
/// Provides screen and window capture capabilities.
/// Screen capture ALWAYS requires user confirmation.
pub struct ScreenCaptureTool {
    ctx: ScreenContext,
}

impl ScreenCaptureTool {
    /// Create a new ScreenCaptureTool with the given context
    pub fn new(ctx: ScreenContext) -> Self {
        Self { ctx }
    }

    /// List available monitors
    fn list_monitors(&self) -> Result<ToolResult> {
        let monitors = xcap::Monitor::all()
            .map_err(|e| AetherError::other(format!("Failed to list monitors: {}", e)))?;

        let result: Vec<serde_json::Value> = monitors
            .iter()
            .enumerate()
            .map(|(idx, m)| {
                json!({
                    "index": idx,
                    "name": m.name().unwrap_or_default(),
                    "width": m.width().unwrap_or(0),
                    "height": m.height().unwrap_or(0),
                    "is_primary": m.is_primary().unwrap_or(false),
                })
            })
            .collect();

        let content = format!("Found {} monitor(s)", monitors.len());

        Ok(ToolResult::success_with_data(content, json!(result)))
    }

    /// List visible windows
    fn list_windows(&self) -> Result<ToolResult> {
        let windows = xcap::Window::all()
            .map_err(|e| AetherError::other(format!("Failed to list windows: {}", e)))?;

        let visible: Vec<_> = windows
            .iter()
            .filter(|w| !w.is_minimized().unwrap_or(true))
            .collect();

        let result: Vec<serde_json::Value> = visible
            .iter()
            .map(|w| {
                json!({
                    "title": w.title().unwrap_or_default(),
                    "app_name": w.app_name().unwrap_or_default(),
                    "width": w.width().unwrap_or(0),
                    "height": w.height().unwrap_or(0),
                })
            })
            .collect();

        let content = format!("Found {} visible window(s)", visible.len());

        Ok(ToolResult::success_with_data(content, json!(result)))
    }

    /// Capture screen
    fn capture_screen(&self, monitor_idx: usize) -> Result<ToolResult> {
        let monitors = xcap::Monitor::all()
            .map_err(|e| AetherError::other(format!("Failed to list monitors: {}", e)))?;

        let monitor = monitors.get(monitor_idx).ok_or_else(|| {
            AetherError::invalid_config(format!("Monitor {} not found", monitor_idx))
        })?;

        let image = monitor
            .capture_image()
            .map_err(|e| AetherError::other(format!("Failed to capture screen: {}", e)))?;

        let resized = self.ctx.resize_if_needed(image);
        let (base64_data, width, height) = self.ctx.encode_image(resized)?;

        Ok(ToolResult::success_with_data(
            format!("Captured screen ({}x{})", width, height),
            json!({
                "width": width,
                "height": height,
                "format": "jpeg",
                "data_base64": base64_data,
                "data_uri": format!("data:image/jpeg;base64,{}", base64_data),
            }),
        ))
    }

    /// Capture window
    fn capture_window(&self, title_filter: &str) -> Result<ToolResult> {
        let windows = xcap::Window::all()
            .map_err(|e| AetherError::other(format!("Failed to list windows: {}", e)))?;

        let window = windows
            .iter()
            .filter(|w| !w.is_minimized().unwrap_or(true))
            .find(|w| {
                w.title()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(&title_filter.to_lowercase())
            })
            .ok_or_else(|| {
                AetherError::invalid_config(format!(
                    "Window with title '{}' not found",
                    title_filter
                ))
            })?;

        let image = window
            .capture_image()
            .map_err(|e| AetherError::other(format!("Failed to capture window: {}", e)))?;

        let resized = self.ctx.resize_if_needed(image);
        let (base64_data, width, height) = self.ctx.encode_image(resized)?;

        let window_title = window.title().unwrap_or_default();

        Ok(ToolResult::success_with_data(
            format!("Captured window '{}' ({}x{})", window_title, width, height),
            json!({
                "width": width,
                "height": height,
                "format": "jpeg",
                "window_title": window_title,
                "data_base64": base64_data,
                "data_uri": format!("data:image/jpeg;base64,{}", base64_data),
            }),
        ))
    }
}

#[async_trait]
impl AgentTool for ScreenCaptureTool {
    fn name(&self) -> &str {
        "screen_capture"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "screen_capture",
            "Capture screen, window, or list available monitors/windows.",
            json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "enum": ["screen", "window", "monitors", "windows"],
                        "description": "What to capture or list (default: screen)",
                        "default": "screen"
                    },
                    "monitor_index": {
                        "type": "integer",
                        "description": "Monitor index for screen capture (default: 0 = primary)"
                    },
                    "window_title": {
                        "type": "string",
                        "description": "Window title substring to match (for window capture)"
                    }
                }
            }),
            ToolCategory::Screen,
        )
        .with_confirmation(true) // Always requires confirmation
    }

    async fn execute(&self, args: &str) -> Result<ToolResult> {
        if !self.ctx.is_enabled() {
            return Ok(ToolResult::error(
                "Screen capture is disabled. Enable it in configuration to use this tool.",
            ));
        }

        // Parse parameters
        let params: ScreenCaptureParams =
            serde_json::from_str(args).unwrap_or(ScreenCaptureParams {
                target: "screen".to_string(),
                monitor_index: None,
                window_title: None,
            });

        match params.target.as_str() {
            "monitors" => self.list_monitors(),
            "windows" => self.list_windows(),
            "screen" => {
                let idx = params.monitor_index.unwrap_or(0);
                self.capture_screen(idx)
            }
            "window" => {
                let title = params.window_title.as_deref().unwrap_or("");
                self.capture_window(title)
            }
            _ => Ok(ToolResult::error(format!(
                "Unknown target: {}. Use 'screen', 'window', 'monitors', or 'windows'.",
                params.target
            ))),
        }
    }

    fn requires_confirmation(&self) -> bool {
        // Screen capture ALWAYS requires confirmation for privacy
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Screen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_tool() -> ScreenCaptureTool {
        let ctx = ScreenContext::new(ScreenConfig::default());
        ScreenCaptureTool::new(ctx)
    }

    fn create_disabled_tool() -> ScreenCaptureTool {
        let ctx = ScreenContext::new(ScreenConfig::disabled());
        ScreenCaptureTool::new(ctx)
    }

    #[tokio::test]
    async fn test_screen_capture_disabled() {
        let tool = create_disabled_tool();

        let args = json!({ "target": "screen" }).to_string();
        let result = tool.execute(&args).await.unwrap();

        assert!(!result.success);
        assert!(result.error.as_ref().unwrap().contains("disabled"));
    }

    #[test]
    fn test_screen_capture_metadata() {
        let tool = create_test_tool();

        assert_eq!(tool.name(), "screen_capture");
        assert!(tool.requires_confirmation());
        assert_eq!(tool.category(), ToolCategory::Screen);
    }

    #[test]
    fn test_screen_capture_definition() {
        let tool = create_test_tool();
        let def = tool.definition();

        assert_eq!(def.name, "screen_capture");
        assert!(def.requires_confirmation);
        assert_eq!(def.category, ToolCategory::Screen);
    }

    #[test]
    fn test_config_default() {
        let config = ScreenConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_dimension, 1920);
        assert_eq!(config.jpeg_quality, 85);
    }

    #[test]
    fn test_config_disabled() {
        let config = ScreenConfig::disabled();
        assert!(!config.enabled);
    }

    // Note: Actual capture tests require a display and are platform-specific
    // They should be run manually or in integration tests
}
