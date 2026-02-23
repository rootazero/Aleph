use crate::error::{AlephError, Result};

use super::{PerceptionSnapshot, SnapshotCaptureArgs};

pub async fn capture_snapshot(_: SnapshotCaptureArgs) -> Result<PerceptionSnapshot> {
    Err(AlephError::tool(
        "snapshot_capture is only supported on macOS",
    ))
}
