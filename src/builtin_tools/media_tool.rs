//! Media tool — camera capture and audio device management.
//!
//! Delegates to `DesktopPlatform::media()` (`MediaCapability`).
//! When the capability is absent, all operations return a friendly message.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::approval::{ActionRequest, ActionType, ApprovalDecision, ApprovalPolicy};
use crate::error::Result;
use crate::tools::AlephTool;

/// Media tool — gives the AI agent access to camera and audio device management.
#[derive(Clone)]
pub struct MediaTool {
    platform: Arc<dyn aleph_desktop::DesktopPlatform>,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
}

impl MediaTool {
    pub fn new(platform: Arc<dyn aleph_desktop::DesktopPlatform>) -> Self {
        Self {
            platform,
            approval_policy: None,
        }
    }

    /// Attach an approval policy to gate camera/microphone capture.
    ///
    /// Capture actions (`camera_snap` / `camera_clip` / `record_audio`) turn on
    /// a sensor and are checked before execution; read-only actions
    /// (`list_audio_devices`, `speech_to_text` over an existing file) always
    /// proceed. Without a policy, capture proceeds as before (byte-identical).
    #[must_use]
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }

    /// Camera/mic capture actions that turn on a sensor.
    fn is_capture_action(action: &str) -> bool {
        matches!(action, "camera_snap" | "camera_clip" | "record_audio")
    }

    /// Check the approval policy for a capture action. Returns `Some(refusal)`
    /// when denied or awaiting confirmation, `None` when allowed (or the action
    /// is read-only, or no policy is configured).
    async fn check_capture_approval(
        &self,
        action: &str,
        target: String,
        display_target: String,
    ) -> Option<MediaOutput> {
        if !Self::is_capture_action(action) {
            return None;
        }
        let policy = self.approval_policy.as_ref()?;

        let (agent_id, context) =
            crate::approval::audit_identity("media", action, &display_target);
        let request = ActionRequest {
            action_type: ActionType::MediaCapture,
            target,
            display_target,
            agent_id,
            context,
            timestamp: chrono::Utc::now(),
        };

        match policy.check(&request).await {
            ApprovalDecision::Allow => {
                policy.record(&request, &ApprovalDecision::Allow).await;
                None
            }
            ApprovalDecision::Deny { reason } => {
                let decision = ApprovalDecision::Deny {
                    reason: reason.clone(),
                };
                policy.record(&request, &decision).await;
                Some(MediaOutput {
                    success: false,
                    data: None,
                    message: Some(format!("Action denied by approval policy: {reason}")),
                })
            }
            ApprovalDecision::Ask { prompt } => Some(MediaOutput {
                success: false,
                data: Some(serde_json::json!({
                    "approval_required": true,
                    "prompt": prompt,
                })),
                message: Some(format!("Approval required: {prompt}")),
            }),
        }
    }
}

/// Arguments for the media tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MediaArgs {
    /// Action to perform: "`camera_snap`", "`camera_clip`", "`record_audio`", "`list_audio_devices`", "`speech_to_text`"
    pub action: String,
    /// JPEG quality (0.0–1.0). Used by `camera_snap`. Default: 0.9
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<f32>,
    /// Recording duration in seconds. `camera_clip`: 0.25–60 (default 3.0);
    /// `record_audio`: 0.25–300 (default 5.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    /// Include audio from microphone. Used by `camera_clip`. Default: false
    #[serde(skip_serializing_if = "Option::is_none")]
    pub with_audio: Option<bool>,
    /// Path to an audio file. Used by `speech_to_text`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_path: Option<String>,
    /// Recognition language (e.g., "en-US", "zh-Hans"). Used by `speech_to_text`. Default: "en-US"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
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
    const DESCRIPTION: &'static str = r#"Camera capture, audio recording, device management, and speech-to-text.

Actions:
- camera_snap: Take a photo from the default camera. Returns base64-encoded JPEG. Optional: quality (0.0–1.0)
- camera_clip: Record a video clip from the default camera. Returns file path to MP4. Optional: duration (seconds, 0.25–60), with_audio (bool)
- record_audio: Record audio from the default microphone. Returns file path to M4A. Optional: duration (seconds, 0.25–300, default 5)
- list_audio_devices: List all audio input devices with names, UIDs, and default status
- speech_to_text: Transcribe an audio file to text using on-device speech recognition. Required: audio_path. Optional: language (default "en-US")

Examples:
{"action":"camera_snap"}
{"action":"camera_snap","quality":0.8}
{"action":"camera_clip","duration":5.0,"with_audio":true}
{"action":"record_audio","duration":10.0}
{"action":"list_audio_devices"}
{"action":"speech_to_text","audio_path":"/path/to/audio.m4a","language":"en-US"}"#;

    type Args = MediaArgs;
    type Output = MediaOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Gate camera/mic capture before touching the sensor. Read-only actions
        // (device list / STT over a file) and the no-policy path fall straight
        // through.
        let (capture_target, capture_display) = if Self::is_capture_action(&args.action) {
            (
                match args.action.as_str() {
                    "camera_snap" => serde_json::json!({
                        "action": "camera_snap",
                        "quality": args.quality,
                    })
                    .to_string(),
                    "camera_clip" => serde_json::json!({
                        "action": "camera_clip",
                        "duration": args.duration,
                        "with_audio": args.with_audio,
                    })
                    .to_string(),
                    "record_audio" => serde_json::json!({
                        "action": "record_audio",
                        "duration": args.duration,
                    })
                    .to_string(),
                    _ => format!("media {}", args.action),
                },
                format!("media {}", args.action),
            )
        } else {
            (String::new(), String::new())
        };
        if let Some(refusal) = self
            .check_capture_approval(&args.action, capture_target, capture_display)
            .await
        {
            return Ok(refusal);
        }

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

            "record_audio" => {
                let config = aleph_desktop::media_types::AudioRecordConfig {
                    duration_secs: args.duration.unwrap_or(5.0),
                }
                .clamped();

                match media_cap.record_audio(config).await {
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

            "speech_to_text" => {
                let audio_path = match &args.audio_path {
                    Some(p) => p.clone(),
                    None => {
                        return Ok(MediaOutput {
                            success: false,
                            data: None,
                            message: Some(
                                "speech_to_text requires 'audio_path' parameter".into(),
                            ),
                        });
                    }
                };

                let config = aleph_desktop::media_types::SpeechToTextConfig {
                    language: args.language.unwrap_or_else(|| "en-US".to_string()),
                };

                match media_cap.speech_to_text(&audio_path, config).await {
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

            unknown => Ok(MediaOutput {
                success: false,
                data: None,
                message: Some(format!(
                    "Unknown action: '{unknown}'. Valid actions: camera_snap, camera_clip, record_audio, list_audio_devices, speech_to_text"
                )),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::{ApprovalPolicy, ConfigApprovalPolicy, PolicyConfig, PolicyRule};
    use crate::tools::AlephTool;
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};

    struct CapturePolicy {
        captured: Mutex<Vec<ActionRequest>>,
    }

    #[async_trait]
    impl ApprovalPolicy for CapturePolicy {
        async fn check(&self, request: &ActionRequest) -> ApprovalDecision {
            self.captured.lock().unwrap().push(request.clone());
            ApprovalDecision::Allow
        }
        async fn record(&self, _request: &ActionRequest, _decision: &ApprovalDecision) {}
    }

    struct StubMedia;

    #[async_trait]
    impl aleph_desktop::traits::MediaCapability for StubMedia {
        async fn camera_snap(
            &self,
            _config: aleph_desktop::media_types::CameraSnapConfig,
        ) -> aleph_desktop::Result<aleph_desktop::media_types::CameraSnapResult> {
            Err(aleph_desktop::DesktopError::NotImplemented("stub".into()))
        }
        async fn camera_clip(
            &self,
            _config: aleph_desktop::media_types::CameraClipConfig,
        ) -> aleph_desktop::Result<aleph_desktop::media_types::CameraClipResult> {
            Err(aleph_desktop::DesktopError::NotImplemented("stub".into()))
        }
        async fn record_audio(
            &self,
            _config: aleph_desktop::media_types::AudioRecordConfig,
        ) -> aleph_desktop::Result<aleph_desktop::media_types::AudioRecordResult> {
            Err(aleph_desktop::DesktopError::NotImplemented("stub".into()))
        }
        async fn list_audio_devices(
            &self,
        ) -> aleph_desktop::Result<Vec<aleph_desktop::media_types::AudioDeviceInfo>> {
            Ok(vec![])
        }
        async fn speech_to_text(
            &self,
            _audio_path: &str,
            _config: aleph_desktop::media_types::SpeechToTextConfig,
        ) -> aleph_desktop::Result<aleph_desktop::media_types::SpeechToTextResult> {
            Err(aleph_desktop::DesktopError::NotImplemented("stub".into()))
        }
    }

    struct StubPlatform(StubMedia);

    impl aleph_desktop::DesktopPlatform for StubPlatform {
        fn platform_name(&self) -> &str {
            "stub"
        }
        fn screen(&self) -> Option<&dyn aleph_desktop::traits::ScreenCapability> {
            None
        }
        fn pim(&self) -> Option<&dyn aleph_desktop::traits::PimCapability> {
            None
        }
        fn system(&self) -> Option<&dyn aleph_desktop::traits::SystemCapability> {
            None
        }
        fn automation(&self) -> Option<&dyn aleph_desktop::traits::AutomationCapability> {
            None
        }
        fn permission(&self) -> Option<&dyn aleph_desktop::traits::PermissionCapability> {
            None
        }
        fn media(&self) -> Option<&dyn aleph_desktop::traits::MediaCapability> {
            Some(&self.0)
        }
    }

    fn tool_with_capture() -> (MediaTool, Arc<CapturePolicy>) {
        let policy = Arc::new(CapturePolicy {
            captured: Mutex::new(Vec::new()),
        });
        let dyn_policy: Arc<dyn ApprovalPolicy> = policy.clone();
        let tool = MediaTool::new(Arc::new(StubPlatform(StubMedia)))
            .with_approval_policy(dyn_policy);
        (tool, policy)
    }

    fn deny_policy_blocking(secret_substring: &str) -> Arc<ConfigApprovalPolicy> {
        use std::collections::HashMap;
        let pattern = format!("*{secret_substring}*");
        let policy = ConfigApprovalPolicy::new(PolicyConfig {
            defaults: HashMap::new(),
            allowlist: vec![],
            blocklist: vec![PolicyRule {
                action_type: ActionType::MediaCapture,
                pattern,
            }],
        });
        Arc::new(policy)
    }

    #[tokio::test]
    async fn camera_clip_target_carries_with_audio_and_duration() {
        let (tool, capture) = tool_with_capture();
        let args = MediaArgs {
            action: "camera_clip".into(),
            quality: None,
            duration: Some(7.5),
            with_audio: Some(true),
            audio_path: None,
            language: None,
        };
        let _ = AlephTool::call(&tool, args).await.unwrap();
        let captured = capture.captured.lock().unwrap();
        assert_eq!(captured.len(), 1);
        let req = &captured[0];
        assert_eq!(req.action_type, ActionType::MediaCapture);
        assert!(
            req.target.contains("\"with_audio\":true"),
            "camera_clip target must surface the with_audio flag for blocklist matching, got: {}",
            req.target
        );
        assert!(
            req.target.contains("\"duration\":7.5"),
            "camera_clip target must surface the duration for blocklist matching, got: {}",
            req.target
        );
    }

    #[tokio::test]
    async fn record_audio_target_carries_duration() {
        let (tool, capture) = tool_with_capture();
        let args = MediaArgs {
            action: "record_audio".into(),
            quality: None,
            duration: Some(120.0),
            with_audio: None,
            audio_path: None,
            language: None,
        };
        let _ = AlephTool::call(&tool, args).await.unwrap();
        let captured = capture.captured.lock().unwrap();
        assert_eq!(captured.len(), 1);
        let req = &captured[0];
        assert!(
            req.target.contains("\"duration\":120.0"),
            "record_audio target must surface the duration for blocklist matching, got: {}",
            req.target
        );
    }

    #[tokio::test]
    async fn blocklist_matching_with_audio_true_actually_blocks() {
        let policy = deny_policy_blocking("\"with_audio\":true");
        let tool = MediaTool::new(Arc::new(StubPlatform(StubMedia)))
            .with_approval_policy(policy as Arc<dyn ApprovalPolicy>);
        let args = MediaArgs {
            action: "camera_clip".into(),
            quality: None,
            duration: Some(3.0),
            with_audio: Some(true),
            audio_path: None,
            language: None,
        };
        let out = AlephTool::call(&tool, args).await.unwrap();
        assert!(!out.success);
        let msg = out.message.as_deref().unwrap_or("");
        assert!(
            msg.contains("denied"),
            "expected denial when blocklist matches with_audio=true, got: {msg}"
        );
    }
}
