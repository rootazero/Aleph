// Browser screenshot tool — captures a screenshot of the current page.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::browser::manager::ProfileManager;
use crate::browser::types::ScreenshotOpts;
use crate::builtin_tools::desktop::VisionBridge;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;
use crate::vision::types::ImageFormat;

/// Arguments for the browser_screenshot tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserScreenshotArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Capture the full page (default: false, captures viewport only).
    #[serde(default)]
    pub full_page: bool,
    /// Augment the screenshot with a text layer a text-only model can read: an
    /// `ocr_text` layer (offline) plus a scene `description` when a vision model
    /// is configured. Pass `true` if you cannot see images. Default: false.
    #[serde(default)]
    pub describe: Option<bool>,
}

/// Output from the browser_screenshot tool.
#[derive(Debug, Serialize)]
pub struct BrowserScreenshotOutput {
    pub success: bool,
    pub image_base64: Option<String>,
    pub message: Option<String>,
    /// Flat OCR text from the page (only when `describe: true` and a provider
    /// produced it). Lets a text-only model read on-screen text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ocr_text: Option<String>,
    /// Multimodal scene description (only when `describe: true` and a vision
    /// model is configured).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Captures a screenshot of the current page.
#[derive(Clone)]
pub struct BrowserScreenshotTool {
    manager: Arc<ProfileManager>,
    /// Optional vision bridge powering `describe: true`. `None` → the describe
    /// flag is ignored and the output is byte-identical to the legacy shape.
    vision_bridge: Option<Arc<VisionBridge>>,
}

impl BrowserScreenshotTool {
    pub fn new(manager: Arc<ProfileManager>) -> Self {
        Self {
            manager,
            vision_bridge: None,
        }
    }

    /// Attach a vision bridge so `screenshot` calls that pass `describe: true`
    /// gain an OCR (and, with a multimodal provider, scene-description) layer.
    pub fn with_vision_bridge(mut self, bridge: Arc<VisionBridge>) -> Self {
        self.vision_bridge = Some(bridge);
        self
    }
}

#[async_trait]
impl AlephTool for BrowserScreenshotTool {
    const NAME: &'static str = "browser_screenshot";
    const DESCRIPTION: &'static str = "Take a screenshot of the current browser page. Pass describe:true if you cannot see images — the result then also carries an ocr_text layer (and a scene description when a vision model is configured) so a text-only model can read the page.";
    type Args = BrowserScreenshotArgs;
    type Output = BrowserScreenshotOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        match super::make_backend_and_tab_guarded(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => {
                let opts = ScreenshotOpts {
                    full_page: args.full_page,
                    ..Default::default()
                };
                match backend.screenshot(&tab_id, opts).await {
                    Ok(result) => {
                        use base64::Engine as _;
                        let image_base64 =
                            base64::engine::general_purpose::STANDARD.encode(&result.png_bytes);

                        // Optional text layer for text-only models. Reuses the
                        // shared VisionBridge (TTL-cached) — identical to the
                        // desktop `screenshot {describe:true}` path. The screenshot
                        // is always PNG (see ScreenshotOpts default format).
                        let (ocr_text, description) = match (&self.vision_bridge, args.describe) {
                            (Some(bridge), Some(true)) => {
                                let aug =
                                    bridge.augment(&image_base64, ImageFormat::Png, true).await;
                                (aug.ocr_text, aug.description)
                            }
                            _ => (None, None),
                        };

                        Ok(BrowserScreenshotOutput {
                            success: true,
                            image_base64: Some(image_base64),
                            message: Some(format!(
                                "Screenshot captured in profile '{}'",
                                args.profile
                            )),
                            ocr_text,
                            description,
                        })
                    }
                    Err(e) => Ok(BrowserScreenshotOutput {
                        success: false,
                        image_base64: None,
                        message: Some(format!("Screenshot failed: {e}")),
                        ocr_text: None,
                        description: None,
                    }),
                }
            }
            Err(e) => Ok(BrowserScreenshotOutput {
                success: false,
                image_base64: None,
                message: Some(format!("{e}")),
                ocr_text: None,
                description: None,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;

    #[tokio::test]
    async fn test_screenshot_default_args() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserScreenshotTool::new(manager);

        let result = tool
            .call(BrowserScreenshotArgs {
                profile: "default".into(),
                full_page: false,
                describe: None,
            })
            .await
            .unwrap();

        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());
    }

    #[tokio::test]
    async fn test_screenshot_full_page() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserScreenshotTool::new(manager);

        let result = tool
            .call(BrowserScreenshotArgs {
                profile: "default".into(),
                full_page: true,
                describe: Some(true),
            })
            .await
            .unwrap();

        // Without a running browser, tools degrade gracefully — describe never
        // runs because there is no successful capture to augment.
        assert!(!result.success);
        assert!(result.message.is_some());
        assert!(result.ocr_text.is_none());
        assert!(result.description.is_none());
    }
}
