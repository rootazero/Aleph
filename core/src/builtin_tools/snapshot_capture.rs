//! Snapshot capture tool for system perception.

use async_trait::async_trait;
use crate::builtin_tools::{notify_tool_result, notify_tool_start};
use crate::error::{AlephError, Result};
use crate::perception::{capture_snapshot, PerceptionSnapshot, SnapshotCaptureArgs};
use crate::tools::AlephTool;

/// SnapshotTool wrapper for AlephTool.
#[derive(Clone, Default)]
pub struct SnapshotCaptureTool;

#[async_trait]
impl AlephTool for SnapshotCaptureTool {
    const NAME: &'static str = "snapshot_capture";
    const DESCRIPTION: &'static str =
        "Capture a system snapshot (AX tree + optional vision OCR) with focus hints.";

    type Args = SnapshotCaptureArgs;
    type Output = PerceptionSnapshot;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let args_summary = format!("target={:?}", args.target.unwrap_or_default());
        notify_tool_start(Self::NAME, &args_summary);

        match capture_snapshot(args).await {
            Ok(snapshot) => {
                let summary = if let Some(errors) = snapshot.errors.as_ref() {
                    if errors
                        .iter()
                        .any(|code| code == "SCREEN_RECORDING_REQUIRED")
                    {
                        "snapshot captured (SCREEN_RECORDING_REQUIRED)".to_string()
                    } else if errors.is_empty() {
                        "snapshot captured".to_string()
                    } else {
                        format!("snapshot captured (partial: {})", errors.join(","))
                    }
                } else {
                    "snapshot captured".to_string()
                };
                notify_tool_result(Self::NAME, &summary, true);
                Ok(snapshot)
            }
            Err(err) => {
                notify_tool_result(Self::NAME, "snapshot failed", false);
                Err(AlephError::tool(format!("snapshot_capture failed: {}", err)))
            }
        }
    }
}

 
