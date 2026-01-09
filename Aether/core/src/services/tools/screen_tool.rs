//! Screen Capture Tool
//!
//! Provides screen and window capture capabilities using xcap.
//! ALWAYS requires user confirmation due to privacy sensitivity.

use async_trait::async_trait;
use base64::{engine::general_purpose, Engine as _};
use serde_json::{json, Value};

use super::SystemTool;
use crate::config::ScreenCaptureToolConfig;
use crate::error::{AetherError, Result};
use crate::mcp::types::{McpResource, McpTool, McpToolResult};

/// Screen capture MCP service
///
/// Provides screen and window capture capabilities.
/// ALWAYS requires user confirmation due to privacy sensitivity.
pub struct ScreenCaptureService {
    config: ScreenCaptureToolConfig,
}

impl ScreenCaptureService {
    /// Create a new ScreenCaptureService
    pub fn new(config: ScreenCaptureToolConfig) -> Self {
        Self { config }
    }

    /// Resize image if it exceeds max dimension
    /// Uses xcap's image crate (0.25) types
    fn resize_if_needed(
        &self,
        img: xcap::image::RgbaImage,
    ) -> xcap::image::RgbaImage {
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
    /// Uses xcap's image crate (0.25) types
    fn encode_image(&self, img: xcap::image::RgbaImage) -> Result<(String, u32, u32)> {
        let (w, h) = img.dimensions();

        // Convert RGBA to RGB for JPEG encoding
        let rgb_img: xcap::image::RgbImage =
            xcap::image::DynamicImage::ImageRgba8(img).to_rgb8();

        let mut buffer = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut buffer);

        // Use xcap's image crate encoder
        rgb_img
            .write_to(&mut cursor, xcap::image::ImageFormat::Jpeg)
            .map_err(|e| AetherError::other(format!("Failed to encode image: {}", e)))?;

        let base64_data = general_purpose::STANDARD.encode(&buffer);

        Ok((base64_data, w, h))
    }
}

impl Default for ScreenCaptureService {
    fn default() -> Self {
        Self::new(ScreenCaptureToolConfig::default())
    }
}

#[async_trait]
impl SystemTool for ScreenCaptureService {
    fn name(&self) -> &str {
        "builtin:screen"
    }

    fn description(&self) -> &str {
        "Screen and window capture"
    }

    async fn list_resources(&self) -> Result<Vec<McpResource>> {
        Ok(vec![
            McpResource {
                uri: "screen://monitors".to_string(),
                name: "Monitors".to_string(),
                description: Some("List of available monitors".to_string()),
                mime_type: Some("application/json".to_string()),
            },
            McpResource {
                uri: "screen://windows".to_string(),
                name: "Windows".to_string(),
                description: Some("List of visible windows".to_string()),
                mime_type: Some("application/json".to_string()),
            },
        ])
    }

    async fn read_resource(&self, uri: &str) -> Result<String> {
        match uri {
            "screen://monitors" => {
                let monitors = xcap::Monitor::all()
                    .map_err(|e| AetherError::other(format!("Failed to list monitors: {}", e)))?;

                let result: Vec<Value> = monitors
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

                Ok(serde_json::to_string_pretty(&result)?)
            }
            "screen://windows" => {
                let windows = xcap::Window::all()
                    .map_err(|e| AetherError::other(format!("Failed to list windows: {}", e)))?;

                let result: Vec<Value> = windows
                    .iter()
                    .map(|w| {
                        json!({
                            "title": w.title().unwrap_or_default(),
                            "app_name": w.app_name().unwrap_or_default(),
                            "width": w.width().unwrap_or(0),
                            "height": w.height().unwrap_or(0),
                            "is_minimized": w.is_minimized().unwrap_or(false),
                        })
                    })
                    .collect();

                Ok(serde_json::to_string_pretty(&result)?)
            }
            _ => Err(AetherError::NotFound(uri.to_string())),
        }
    }

    fn list_tools(&self) -> Vec<McpTool> {
        if !self.config.enabled {
            return vec![];
        }

        vec![
            McpTool {
                name: "screen_capture".to_string(),
                description: "Capture screen, window, or region as image".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "target": {
                            "type": "string",
                            "enum": ["screen", "window"],
                            "description": "What to capture (default: screen)",
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
                requires_confirmation: true, // Always requires confirmation
            },
            McpTool {
                name: "list_monitors".to_string(),
                description: "List available monitors".to_string(),
                input_schema: json!({ "type": "object" }),
                requires_confirmation: false,
            },
            McpTool {
                name: "list_windows".to_string(),
                description: "List visible windows".to_string(),
                input_schema: json!({ "type": "object" }),
                requires_confirmation: false,
            },
        ]
    }

    async fn call_tool(&self, name: &str, args: Value) -> Result<McpToolResult> {
        match name {
            "list_monitors" => {
                let monitors = xcap::Monitor::all()
                    .map_err(|e| AetherError::other(format!("Failed to list monitors: {}", e)))?;

                let result: Vec<Value> = monitors
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

                Ok(McpToolResult::success(json!(result)))
            }

            "list_windows" => {
                let windows = xcap::Window::all()
                    .map_err(|e| AetherError::other(format!("Failed to list windows: {}", e)))?;

                let result: Vec<Value> = windows
                    .iter()
                    .filter(|w| !w.is_minimized().unwrap_or(true))
                    .map(|w| {
                        json!({
                            "title": w.title().unwrap_or_default(),
                            "app_name": w.app_name().unwrap_or_default(),
                            "width": w.width().unwrap_or(0),
                            "height": w.height().unwrap_or(0),
                        })
                    })
                    .collect();

                Ok(McpToolResult::success(json!(result)))
            }

            "screen_capture" => {
                let target = args
                    .get("target")
                    .and_then(|v| v.as_str())
                    .unwrap_or("screen");

                let image = match target {
                    "screen" => {
                        let monitor_idx = args
                            .get("monitor_index")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as usize;

                        let monitors = xcap::Monitor::all().map_err(|e| {
                            AetherError::other(format!("Failed to list monitors: {}", e))
                        })?;

                        let monitor = monitors.get(monitor_idx).ok_or_else(|| {
                            AetherError::invalid_config(format!("Monitor {} not found", monitor_idx))
                        })?;

                        monitor.capture_image().map_err(|e| {
                            AetherError::other(format!("Failed to capture screen: {}", e))
                        })?
                    }

                    "window" => {
                        let title_filter = args
                            .get("window_title")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");

                        let windows = xcap::Window::all().map_err(|e| {
                            AetherError::other(format!("Failed to list windows: {}", e))
                        })?;

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

                        window.capture_image().map_err(|e| {
                            AetherError::other(format!("Failed to capture window: {}", e))
                        })?
                    }

                    _ => {
                        return Ok(McpToolResult::error(format!("Unknown target: {}", target)));
                    }
                };

                // Resize if too large
                let resized = self.resize_if_needed(image);

                // Encode to JPEG and base64
                let (base64_data, width, height) = self.encode_image(resized)?;

                Ok(McpToolResult::success(json!({
                    "width": width,
                    "height": height,
                    "format": "jpeg",
                    "data_base64": base64_data,
                    "data_uri": format!("data:image/jpeg;base64,{}", base64_data),
                })))
            }

            _ => Ok(McpToolResult::error(format!("Unknown tool: {}", name))),
        }
    }

    fn requires_confirmation(&self, tool_name: &str) -> bool {
        // Only screen_capture requires confirmation (privacy sensitive)
        // list_monitors and list_windows are metadata only
        matches!(tool_name, "screen_capture")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_screen_capture_requires_confirmation() {
        let service = ScreenCaptureService::default();
        assert!(service.requires_confirmation("screen_capture"));
        assert!(!service.requires_confirmation("list_monitors"));
        assert!(!service.requires_confirmation("list_windows"));
    }

    #[test]
    fn test_disabled_service() {
        let config = ScreenCaptureToolConfig {
            enabled: false,
            ..Default::default()
        };
        let service = ScreenCaptureService::new(config);
        assert!(service.list_tools().is_empty());
    }

    #[test]
    fn test_tool_listing() {
        let service = ScreenCaptureService::default();
        let tools = service.list_tools();
        assert_eq!(tools.len(), 3);

        let screen_capture = tools.iter().find(|t| t.name == "screen_capture").unwrap();
        assert!(screen_capture.requires_confirmation);

        let list_monitors = tools.iter().find(|t| t.name == "list_monitors").unwrap();
        assert!(!list_monitors.requires_confirmation);
    }

    // Note: Actual capture tests require a display and are platform-specific
    // They should be run manually or in integration tests
}
