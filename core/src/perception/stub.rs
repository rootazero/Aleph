use crate::error::{AetherError, Result};

use super::{PerceptionSnapshot, SnapshotCaptureArgs};

pub async fn capture_snapshot(_: SnapshotCaptureArgs) -> Result<PerceptionSnapshot> {
    Err(AetherError::tool(
        "snapshot_capture is only supported on macOS",
    ))
}
