//! Media tool — camera capture and audio device management.
//!
//! Delegates to `DesktopPlatform::media()` (MediaCapability).
//! When the capability is absent, all operations return a friendly message.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;
use crate::tools::AlephTool;

/// Media tool — gives the AI agent access to camera and audio device management.
#[derive(Clone)]
pub struct MediaTool {
    platform: Arc<dyn aleph_desktop::DesktopPlatform>,
}

impl MediaTool {
    pub fn new(platform: Arc<dyn aleph_desktop::DesktopPlatform>) -> Self {
        Self { platform }
    }
}

/// Arguments for the media tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MediaArgs {
    /// Action to perform: "camera_snap", "camera_clip", "list_audio_devices"
    pub action: String,
    /// JPEG quality (0.0–1.0). Used by camera_snap. Default: 0.9
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<f32>,
    /// Recording duration in seconds (0.25–60.0). Used by camera_clip. Default: 3.0
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    /// Include audio from microphone. Used by camera_clip. Default: false
    #[serde(skip_serializing_if = "Option::is_none")]
    pub with_audio: Option<bool>,
}

/// Output from the media tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaOutput {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[async_trait]
impl AlephTool for MediaTool {
    const NAME: &'static str = "media";
    const DESCRIPTION: &'static str = r#"Camera capture and audio device management.

Actions:
- camera_snap: Take a photo from the default camera. Returns base64-encoded JPEG. Optional: quality (0.0–1.0)
- camera_clip: Record a video clip from the default camera. Returns file path to MP4. Optional: duration (seconds, 0.25–60), with_audio (bool)
- list_audio_devices: List all audio input devices with names, UIDs, and default status

Examples:
{"action":"camera_snap"}
{"action":"camera_snap","quality":0.8}
{"action":"camera_clip","duration":5.0,"with_audio":true}
{"action":"list_audio_devices"}"#;

    type Args = MediaArgs;
    type Output = MediaOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let media_cap = match self.platform.media() {
            Some(m) => m,
            None => {
                return Ok(MediaOutput {
                    success: false,
                    data: None,
                    message: Some(format!(
                        "Media capability is not available on {}. \
                         Camera and audio device management require macOS.",
                        self.platform.platform_name()
                    )),
                });
            }
        };

        match args.action.as_str() {
            "camera_snap" => {
                let config = aleph_desktop::media_types::CameraSnapConfig {
                    quality: args.quality.unwrap_or(0.9),
                }
                .clamped();

                match media_cap.camera_snap(config).await {
                    Ok(result) => Ok(MediaOutput {
                        success: true,
                        data: Some(serde_json::to_value(result).unwrap_or_default()),
                        message: None,
                    }),
                    Err(e) => Ok(MediaOutput {
                        success: false,
                        data: None,
                        message: Some(e.to_string()),
                    }),
                }
            }

            "camera_clip" => {
                let config = aleph_desktop::media_types::CameraClipConfig {
                    duration_secs: args.duration.unwrap_or(3.0),
                    with_audio: args.with_audio.unwrap_or(false),
                }
                .clamped();

                match media_cap.camera_clip(config).await {
                    Ok(result) => Ok(MediaOutput {
                        success: true,
                        data: Some(serde_json::to_value(result).unwrap_or_default()),
                        message: None,
                    }),
                    Err(e) => Ok(MediaOutput {
                        success: false,
                        data: None,
                        message: Some(e.to_string()),
                    }),
                }
            }

            "list_audio_devices" => match media_cap.list_audio_devices().await {
                Ok(devices) => Ok(MediaOutput {
                    success: true,
                    data: Some(serde_json::to_value(devices).unwrap_or_default()),
                    message: None,
                }),
                Err(e) => Ok(MediaOutput {
                    success: false,
                    data: None,
                    message: Some(e.to_string()),
                }),
            },

            unknown => Ok(MediaOutput {
                success: false,
                data: None,
                message: Some(format!(
                    "Unknown action: '{unknown}'. Valid actions: camera_snap, camera_clip, list_audio_devices"
                )),
            }),
        }
    }
}
