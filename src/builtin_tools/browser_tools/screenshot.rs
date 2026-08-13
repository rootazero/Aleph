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

/// Arguments for the `browser_screenshot` tool.
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

/// Maximum characters of vision-model `description` handed back to the model.
///
/// A scene description is prose about one screen; anything longer than this is
/// not a description, it is a hostile page having talked the vision model into
/// relaying a wall of text. Much tighter than the page-read budget
/// [`DEFAULT_CONTENT_MAX_CHARS`](super::DEFAULT_CONTENT_MAX_CHARS) that
/// `ocr_text` gets, because OCR legitimately scales with the page.
const MAX_DESCRIPTION_CHARS: usize = 4_000;

/// Route the two `describe: true` text layers through the module egress
/// chokepoint. Both are page-derived — i.e. attacker-controlled — and were the
/// only page reads in this module that skipped it entirely: no redaction, no
/// injection fence, no size bound.
///
/// The two get different budgets but the same wrapper, and the difference is
/// deliberate:
///
/// - `ocr_text` is verbatim on-screen text, so it is a page read like any
///   other — full [`redact_and_wrap`](super::redact_and_wrap): the page-sized
///   char budget, secret redaction, injection fence.
/// - `description` is a vision model's prose *about* the page rather than a
///   transcript, but a page that paints "ignore previous instructions" gets it
///   relayed verbatim by any model asked to describe what it sees, so it earns
///   the same fence. What legitimately differs is the size: one screen's worth
///   of prose, not a page. It is bounded at [`MAX_DESCRIPTION_CHARS`] and then
///   handed to [`redact_wrap`](super::redact_wrap), the already-bounded entry
///   point `browser_snapshot` uses.
fn wrap_describe_layers(
    manager: &ProfileManager,
    ocr_text: Option<String>,
    description: Option<String>,
) -> (Option<String>, Option<String>) {
    (
        ocr_text.map(|t| super::redact_and_wrap(manager, &t)),
        description.map(|t| {
            let (bounded, _) = super::bound_content(&t, MAX_DESCRIPTION_CHARS);
            super::redact_wrap(manager, &bounded)
        }),
    )
}

/// Output from the `browser_screenshot` tool.
#[derive(Debug, Serialize)]
pub struct BrowserScreenshotOutput {
    pub success: bool,
    pub image_base64: Option<String>,
    /// Encoding of `image_base64` — always `"png"` (see the `ScreenshotOpts`
    /// default). Not decoration: `result_processing::extract_image_in_place`
    /// refuses to hoist an inline image without a recognised `format`, so
    /// without this field the base64 stayed in the TEXT channel and the result
    /// budget truncated it into an undecodable fragment — the model acted on a
    /// screen it never saw. `desktop`'s screenshot output has always carried it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    pub message: Option<String>,
    /// Flat OCR text from the page (only when `describe: true` and a provider
    /// produced it). Lets a text-only model read on-screen text. Page-derived,
    /// so it leaves through the module egress chokepoint like every other page
    /// read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ocr_text: Option<String>,
    /// Multimodal scene description (only when `describe: true` and a vision
    /// model is configured). Also page-derived — see the wrapping note in
    /// `call`.
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
    pub const fn new(manager: Arc<ProfileManager>) -> Self {
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
                        // Bound the pixel budget before it reaches the model —
                        // the screenshot twin of the text-read char cap. Stays
                        // PNG, so the base64 output and vision-bridge contract
                        // below are unchanged.
                        let png_bytes = super::bound_screenshot_png(result.png_bytes);
                        let image_base64 =
                            base64::engine::general_purpose::STANDARD.encode(&png_bytes);

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

                        let (ocr_text, description) =
                            wrap_describe_layers(&self.manager, ocr_text, description);

                        Ok(BrowserScreenshotOutput {
                            success: true,
                            image_base64: Some(image_base64),
                            format: Some("png".into()),
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
                        format: None,
                        message: Some(format!(
                            "Screenshot failed: {}",
                            super::backend_error_text(&self.manager, &e)
                        )),
                        ocr_text: None,
                        description: None,
                    }),
                }
            }
            Err(e) => Ok(BrowserScreenshotOutput {
                success: false,
                image_base64: None,
                format: None,
                message: Some(super::backend_error_text(&self.manager, &e)),
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

    /// The perceive→act loop: without a `format` key
    /// `result_processing::extract_image_in_place` bails out and the base64
    /// stays in the text channel, where the result budget truncates it into an
    /// undecodable fragment. `desktop`'s screenshot output has always carried
    /// the key; this asserts browser's does too.
    #[test]
    fn screenshot_output_is_hoisted_as_a_real_image_block() {
        let out = BrowserScreenshotOutput {
            success: true,
            // > 256 chars, so the hoister treats it as an image and not a marker.
            image_base64: Some("A".repeat(5_000)),
            format: Some("png".into()),
            message: Some("Screenshot captured in profile 'default'".into()),
            ocr_text: None,
            description: None,
        };
        let mut value = serde_json::to_value(&out).expect("output serializes");
        let images = crate::tools::result_processing::hoist_inline_images(&mut value);

        assert_eq!(
            images.len(),
            1,
            "the screenshot must reach the model as an image block"
        );
        assert_eq!(images[0].mime_type, "image/png");
        assert_eq!(images[0].data.len(), 5_000);
        // …and the budget-blowing blob is gone from the text channel.
        assert!(value["image_base64"].as_str().unwrap().len() < 256);
    }

    /// `describe: true` returns page-derived text. Until now it was the only
    /// page read in the module that reached the model unredacted, unfenced and
    /// unbounded.
    #[test]
    fn describe_layers_go_through_the_egress_chokepoint() {
        use crate::security::content_sanitizer::split_external_fence;
        let manager = ProfileManager::new(BrowserSystemConfig::default());

        // A "description" long enough to be an injected relay, not a description.
        let flood = "x".repeat(MAX_DESCRIPTION_CHARS * 3);
        let (ocr, desc) = wrap_describe_layers(
            &manager,
            Some("Signed in. token sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789".into()),
            Some(flood),
        );

        let ocr = ocr.expect("ocr layer");
        assert!(!ocr.contains("sk-ant-api03"), "secret survived: {ocr}");
        assert!(ocr.contains("[REDACTED:"));
        assert!(
            split_external_fence(&ocr).is_some(),
            "OCR text must be wholly fenced: {ocr}"
        );

        let desc = desc.expect("description layer");
        let fenced = split_external_fence(&desc).expect("description must be wholly fenced");
        assert!(
            fenced.interior.chars().count() <= MAX_DESCRIPTION_CHARS,
            "description is not bounded: {} chars",
            fenced.interior.chars().count()
        );
    }
}
