// Browser record tool — records the current tab to a video file (C2; spec:
// docs/superpowers/specs/2026-09-27-browser-recording-design.md §4).

use std::path::PathBuf;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::approval::{ActionType, ApprovalPolicy};
use crate::browser::cdp_backend::recording::{
    RecordStartOptions, RecordingReceipt, RecordingStatus,
};
use crate::browser::manager::ProfileManager;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// The fps-cap bounds (spec §4: 1–60). The cap shapes the container
/// timestamps; the screencast stream itself is repaint-driven, so this is an
/// upper label, not a promise — the receipt's frame counts are the truth.
const MIN_FPS: u32 = 1;
const MAX_FPS: u32 = 60;

/// JPEG quality bounds for the wire frames (CDP's own range).
const MAX_QUALITY: u8 = 100;

/// Which `browser_record` operation to perform. **No default** — an
/// action-less call is a model typo, and guessing `start` for it would open
/// a recording nobody asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RecordAction {
    /// Start recording the active tab. Optional `path` (extension chooses
    /// the codec: .webm/.mp4), `fps` (1-60, default 10), `quality` (1-100,
    /// default 80).
    Start,
    /// Stop and return the verified receipt. Optional `recording_id` stops
    /// by id instead of by the active tab.
    Stop,
    /// The live view of the active tab's recording (or a plain "none").
    Status,
}

/// Arguments for the `browser_record` tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserRecordArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Which operation to perform — REQUIRED, no default.
    pub action: RecordAction,
    /// `stop`: the recording id (`rec1`, …) as start/status reported it.
    /// When absent, the active tab's recording is stopped.
    #[serde(default)]
    pub recording_id: Option<String>,
    /// `start`: output file; the extension chooses the codec. Default: a
    /// managed path under ~/.aleph/recordings/<profile>/.
    #[serde(default)]
    pub path: Option<String>,
    /// `start`: frames-per-second cap (1-60, default 10).
    #[serde(default)]
    pub fps: Option<u32>,
    /// `start`: JPEG quality of the wire frames (1-100, default 80).
    #[serde(default)]
    pub quality: Option<u8>,
}

/// One validated call, with the model's raw optionality resolved away.
enum RecordRequest {
    Start {
        options: RecordStartOptions,
        /// The approval gate's target: the path the model spelled, or the
        /// fact that it asked for the managed default.
        display_target: String,
    },
    Stop {
        recording_id: Option<String>,
    },
    Status,
}

impl BrowserRecordArgs {
    /// Validate BEFORE the approval gate: a malformed call is a model mistake
    /// and must not consume a user approval (the ordering `browser_click` and
    /// `browser_network` both state). Errors are messages, not `Err` — the
    /// family convention for a malformed call.
    fn validate(&self) -> std::result::Result<RecordRequest, String> {
        match self.action {
            RecordAction::Start => {
                let fps = self.fps.unwrap_or(10);
                if !(MIN_FPS..=MAX_FPS).contains(&fps) {
                    return Err(format!(
                        "fps must be {MIN_FPS}-{MAX_FPS} (got {fps}) — the cap shapes the \
                         container timestamps; the receipt's frame counts are the truth"
                    ));
                }
                let quality = self.quality.unwrap_or(80);
                if quality == 0 || quality > MAX_QUALITY {
                    return Err(format!(
                        "quality must be 1-{MAX_QUALITY} (got {quality}) — it is the JPEG \
                         quality of the wire frames"
                    ));
                }
                let path = match self.path.as_deref() {
                    None => None,
                    Some("") => {
                        return Err(
                            "path is empty — drop the key for the managed default under \
                             ~/.aleph/recordings/<profile>/"
                                .to_string(),
                        );
                    }
                    // Second layer of the no-extension refusal (the registry
                    // is the first): the extension chooses the codec, so a
                    // path without one is a caller error, not something to
                    // guess at. Refused here, where the message reaches the
                    // model directly.
                    Some(p) => {
                        let path = PathBuf::from(p);
                        if path.extension().is_none() {
                            return Err(format!(
                                "path has no extension: {p} — the extension chooses the \
                                 codec (.webm → VP8, .mp4 → H.264)"
                            ));
                        }
                        Some(path)
                    }
                };
                let display_target = self
                    .path
                    .clone()
                    .unwrap_or_else(|| "<managed default under ~/.aleph/recordings>".to_string());
                Ok(RecordRequest::Start {
                    options: RecordStartOptions {
                        path,
                        fps_cap: fps,
                        quality,
                        ..RecordStartOptions::default()
                    },
                    display_target,
                })
            }
            RecordAction::Stop => {
                let recording_id = match self.recording_id.as_deref() {
                    None => None,
                    Some("") => {
                        return Err("recording_id is empty — drop it to stop the active tab's \
                             recording, or pass the id start/status reported"
                            .to_string());
                    }
                    Some(id) => Some(id.to_string()),
                };
                Ok(RecordRequest::Stop { recording_id })
            }
            RecordAction::Status => Ok(RecordRequest::Status),
        }
    }
}

/// Output from the `browser_record` tool.
#[derive(Debug, Serialize)]
pub struct BrowserRecordOutput {
    pub success: bool,
    /// The live view (action=start/status).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recording: Option<RecordingStatus>,
    /// The verified receipt (action=stop).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<RecordingReceipt>,
    pub message: Option<String>,
}

impl BrowserRecordOutput {
    fn failed(message: String) -> Self {
        Self {
            success: false,
            recording: None,
            receipt: None,
            message: Some(message),
        }
    }
}

/// Records the current browser tab to a video file (start / stop / status).
#[derive(Clone)]
pub struct BrowserRecordTool {
    manager: Arc<ProfileManager>,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
}

impl BrowserRecordTool {
    pub const fn new(manager: Arc<ProfileManager>) -> Self {
        Self {
            manager,
            approval_policy: None,
        }
    }

    /// Gate `start` behind the approval policy: a start writes a growing
    /// video file and streams the page's pixels through an encoder — the
    /// same family of page-content capture as a screenshot, so Ask is the
    /// matching default. `stop` and `status` skip the gate: an off-switch
    /// and a read stay reachable (判据 §14). With no policy wired the tool
    /// behaves exactly as before.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }
}

#[async_trait]
impl AlephTool for BrowserRecordTool {
    const NAME: &'static str = "browser_record";
    // R9 byte discipline: ≤80 bytes (pinned by a test below). The detail —
    // which action needs which field, the managed default's location, the
    // codec-by-extension rule — lives in the JsonSchema doc comments, not
    // here.
    const DESCRIPTION: &'static str =
        "Record the current tab to a video file (start/stop/status) - cdp profiles only";
    type Args = BrowserRecordArgs;
    type Output = BrowserRecordOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Validate before the approval gate: a malformed call must not
        // consume a user approval (see `validate`).
        let request = match args.validate() {
            Ok(request) => request,
            Err(message) => return Ok(BrowserRecordOutput::failed(message)),
        };
        if let RecordRequest::Start { display_target, .. } = &request {
            if let Some(message) = super::check_browser_approval(
                self.approval_policy.as_ref(),
                ActionType::BrowserRecord,
                "record_start",
                display_target,
            )
            .await
            {
                return Ok(BrowserRecordOutput::failed(message));
            }
        }
        // `start` is a page-content capture (the page's pixels leave the
        // process), so it takes the SSRF-guarded pair exactly like
        // `browser_screenshot` does. `stop`/`status` take the UNGUARDED
        // pair: a stop refused because the current page is redirect-blocked
        // would leave the recording running with no off-switch — fail-dead
        // (判据 §14), and neither action reads page content.
        let pair = match &request {
            RecordRequest::Start { .. } => {
                super::make_backend_and_tab_guarded(&self.manager, &args.profile).await
            }
            RecordRequest::Stop { .. } | RecordRequest::Status => {
                super::make_backend_and_tab(&self.manager, &args.profile).await
            }
        };
        let (backend, tab_id) = match pair {
            Ok(pair) => pair,
            Err(e) => {
                return Ok(BrowserRecordOutput::failed(super::backend_error_text(
                    &self.manager,
                    &e,
                )));
            }
        };
        match request {
            RecordRequest::Start { options, .. } => {
                let fps_cap = options.fps_cap;
                match backend.record_start(&tab_id, options).await {
                    Ok(status) => Ok(BrowserRecordOutput {
                        success: true,
                        message: Some(format!(
                            "recording {} started → {} (fps cap {fps_cap}); stop it with \
                             browser_record{{action:\"stop\"}}",
                            status.recording_id,
                            status.path.display()
                        )),
                        recording: Some(status),
                        receipt: None,
                    }),
                    Err(e) => Ok(BrowserRecordOutput::failed(format!(
                        "record start failed: {}",
                        super::backend_error_text(&self.manager, &e)
                    ))),
                }
            }
            RecordRequest::Stop { recording_id } => {
                match backend.record_stop(&tab_id, recording_id.as_deref()).await {
                    Ok(receipt) => {
                        // The headline a model must not miss: complete vs
                        // truncated. The receipt carries every number; the
                        // message carries the verdict and (when truncated)
                        // the reason.
                        let message = if receipt.complete {
                            format!(
                                "recording {} complete → {} ({} bytes, {} frames in {} ms)",
                                receipt.recording_id,
                                receipt.path.display(),
                                receipt.size_bytes,
                                receipt.encoded_frames,
                                receipt.duration_ms
                            )
                        } else {
                            format!(
                                "recording {} INCOMPLETE → {} (encoder_exit={}, dropped {} frame(s){})",
                                receipt.recording_id,
                                receipt.path.display(),
                                receipt.encoder_exit,
                                receipt.dropped_frames,
                                receipt
                                    .truncation_reason
                                    .as_deref()
                                    .map(|r| format!("; {r}"))
                                    .unwrap_or_default()
                            )
                        };
                        Ok(BrowserRecordOutput {
                            success: receipt.complete,
                            message: Some(message),
                            recording: None,
                            receipt: Some(receipt),
                        })
                    }
                    Err(e) => Ok(BrowserRecordOutput::failed(super::backend_error_text(
                        &self.manager,
                        &e,
                    ))),
                }
            }
            RecordRequest::Status => match backend.record_status(&tab_id).await {
                // "None" is a plain answer, not an error (spec §4: 无录制明确
                // 说无) — the call succeeded; the tab simply is not recording.
                Ok(Some(status)) => Ok(BrowserRecordOutput {
                    success: true,
                    message: Some(format!(
                        "recording {} in progress ({} ms, {} frames captured, {} dropped)",
                        status.recording_id,
                        status.elapsed_ms,
                        status.captured_frames,
                        status.dropped_frames
                    )),
                    recording: Some(status),
                    receipt: None,
                }),
                Ok(None) => Ok(BrowserRecordOutput {
                    success: true,
                    message: Some(format!(
                        "no recording in progress on the active tab of profile '{}'",
                        args.profile
                    )),
                    recording: None,
                    receipt: None,
                }),
                Err(e) => Ok(BrowserRecordOutput::failed(super::backend_error_text(
                    &self.manager,
                    &e,
                ))),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;

    fn tool() -> BrowserRecordTool {
        BrowserRecordTool::new(Arc::new(
            ProfileManager::new(BrowserSystemConfig::default()),
        ))
    }

    fn start_args() -> BrowserRecordArgs {
        BrowserRecordArgs {
            profile: "default".into(),
            action: RecordAction::Start,
            recording_id: None,
            path: None,
            fps: None,
            quality: None,
        }
    }

    /// `action` is REQUIRED — no default. A bare `browser_record{}` is a
    /// model typo and must fail at deserialization, not guess a mutation.
    #[test]
    fn action_is_required_there_is_no_default() {
        let err = serde_json::from_str::<BrowserRecordArgs>("{}")
            .expect_err("an action-less call must not parse");
        assert!(err.to_string().contains("action"), "{err}");
        let args: BrowserRecordArgs =
            serde_json::from_str(r#"{"action":"status"}"#).expect("parses");
        assert_eq!(args.action, RecordAction::Status);
        assert_eq!(args.profile, "default");
    }

    /// The DESCRIPTION ships on every turn's tool listing; the ≤80-byte
    /// discipline is what keeps it there (R9). This is the pin the ratchet
    /// cannot give: the ceiling measures the SUM, this measures the line.
    #[test]
    fn description_stays_within_the_80_byte_discipline() {
        assert!(
            BrowserRecordTool::DESCRIPTION.len() <= 80,
            "DESCRIPTION is {} bytes (max 80): {:?}",
            BrowserRecordTool::DESCRIPTION.len(),
            BrowserRecordTool::DESCRIPTION
        );
        // And it names the three actions — the one thing a model needs
        // before it can even ask for the schema.
        for action in ["start", "stop", "status"] {
            assert!(
                BrowserRecordTool::DESCRIPTION.contains(action),
                "DESCRIPTION does not name `{action}`"
            );
        }
    }

    /// spec §4: fps 1-60. Both walls, each refused at the input side, before
    /// any backend is built (no browser running in these tests).
    #[tokio::test]
    async fn start_rejects_fps_out_of_range() {
        for fps in [0, 61] {
            let mut args = start_args();
            args.fps = Some(fps);
            let result = tool().call(args).await.unwrap();
            assert!(!result.success);
            assert!(
                result.message.as_deref().is_some_and(|m| m.contains("fps")),
                "fps={fps}: got {:?}",
                result.message
            );
        }
        // The walls themselves pass validation (they fail later, on the
        // missing browser — proof the size gate admitted them).
        for fps in [1, 60] {
            let mut args = start_args();
            args.fps = Some(fps);
            let result = tool().call(args).await.unwrap();
            assert!(
                result
                    .message
                    .as_deref()
                    .is_none_or(|m| !m.contains("fps must")),
                "fps={fps} must pass the range gate: {:?}",
                result.message
            );
        }
    }

    /// JPEG quality 1-100 (CDP's own range); 0 and 101 are refused at the
    /// input side.
    #[tokio::test]
    async fn start_rejects_quality_out_of_range() {
        for quality in [0u8, 101] {
            let mut args = start_args();
            args.quality = Some(quality);
            let result = tool().call(args).await.unwrap();
            assert!(!result.success);
            assert!(
                result
                    .message
                    .as_deref()
                    .is_some_and(|m| m.contains("quality")),
                "quality={quality}: got {:?}",
                result.message
            );
        }
    }

    /// Review Focus #4's second layer: the extension chooses the codec, so
    /// a path without one is refused here too (the registry is the first
    /// layer — defense in depth, and here the message reaches the model
    /// directly).
    #[tokio::test]
    async fn start_rejects_a_path_without_an_extension() {
        let mut args = start_args();
        args.path = Some("/tmp/no-extension".into());
        let result = tool().call(args).await.unwrap();
        assert!(!result.success);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("extension")),
            "got: {:?}",
            result.message
        );
    }

    /// click.rs's ordering contract, applied to the record gate: a malformed
    /// call is a model mistake and must not consume a user approval.
    #[tokio::test]
    async fn start_validates_before_the_approval_gate() {
        use crate::approval::{ConfigApprovalPolicy, DefaultDecision, PolicyConfig};
        use std::collections::HashMap;
        let mut defaults = HashMap::new();
        defaults.insert(ActionType::BrowserRecord, DefaultDecision::Deny);
        let policy = Arc::new(ConfigApprovalPolicy::new(PolicyConfig {
            defaults,
            allowlist: vec![],
            blocklist: vec![],
        }));
        let tool = BrowserRecordTool::new(Arc::new(ProfileManager::new(
            BrowserSystemConfig::default(),
        )))
        .with_approval_policy(policy);

        let mut args = start_args();
        args.fps = Some(0);
        let result = tool.call(args).await.unwrap();
        assert!(!result.success);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("fps") && !m.contains("denied")),
            "validation speaks before the gate: {:?}",
            result.message
        );
    }

    /// A policy that denies recording stops the call BEFORE any backend
    /// exists: no browser is contacted to be told no.
    #[tokio::test]
    async fn a_denied_start_never_reaches_the_backend() {
        use crate::approval::{ConfigApprovalPolicy, DefaultDecision, PolicyConfig};
        use std::collections::HashMap;
        let mut defaults = HashMap::new();
        defaults.insert(ActionType::BrowserRecord, DefaultDecision::Deny);
        let policy = Arc::new(ConfigApprovalPolicy::new(PolicyConfig {
            defaults,
            allowlist: vec![],
            blocklist: vec![],
        }));
        let tool = BrowserRecordTool::new(Arc::new(ProfileManager::new(
            BrowserSystemConfig::default(),
        )))
        .with_approval_policy(policy);

        let result = tool.call(start_args()).await.unwrap();
        assert!(!result.success);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("denied")),
            "got: {:?}",
            result.message
        );
    }

    /// `stop` and `status` are the off-switch and a read: they stay OUTSIDE
    /// the gate. Under a Deny policy they must still reach the backend —
    /// where, with no browser running, they degrade exactly like any other
    /// tool, and never with a "denied".
    #[tokio::test]
    async fn stop_and_status_skip_the_gate() {
        use crate::approval::{ConfigApprovalPolicy, DefaultDecision, PolicyConfig};
        use std::collections::HashMap;
        let mut defaults = HashMap::new();
        defaults.insert(ActionType::BrowserRecord, DefaultDecision::Deny);
        let policy = Arc::new(ConfigApprovalPolicy::new(PolicyConfig {
            defaults,
            allowlist: vec![],
            blocklist: vec![],
        }));
        let tool = BrowserRecordTool::new(Arc::new(ProfileManager::new(
            BrowserSystemConfig::default(),
        )))
        .with_approval_policy(policy);

        for action in [RecordAction::Stop, RecordAction::Status] {
            let result = tool
                .call(BrowserRecordArgs {
                    action,
                    ..start_args()
                })
                .await
                .unwrap();
            assert!(
                result
                    .message
                    .as_deref()
                    .is_none_or(|m| !m.contains("denied")),
                "{action:?} must not be gated: {:?}",
                result.message
            );
        }
    }

    /// "Nothing is recording" is a plain fact with its own error text, not a
    /// generic failure: the stop path surfaces `NoActiveRecording`, whose
    /// message names the tab asked about and points at `status` — pinned here
    /// because this text is the tool layer's contract with the model.
    #[test]
    fn stop_without_a_recording_says_so_plainly() {
        let text = crate::browser::error::BrowserError::NoActiveRecording("t1".into()).to_string();
        assert!(text.contains("no recording in progress"), "{text}");
        assert!(text.contains("t1"), "{text}");
        assert!(text.contains("status"), "{text}");
    }

    /// Every action degrades gracefully without a running browser.
    #[tokio::test]
    async fn every_action_degrades_without_a_browser() {
        for action in [
            RecordAction::Start,
            RecordAction::Stop,
            RecordAction::Status,
        ] {
            let result = tool()
                .call(BrowserRecordArgs {
                    action,
                    ..start_args()
                })
                .await
                .unwrap();
            assert!(!result.success, "{action:?}");
            assert!(result.message.is_some(), "{action:?}");
        }
    }
}
