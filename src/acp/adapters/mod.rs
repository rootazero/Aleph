//! Concrete ACP harness adapters for supported CLI tools.

mod custom;
mod generic;

pub use custom::CustomAcpAdapter;
pub use generic::GenericAcpAdapter;

use crate::acp::output_format::OutputFormat;
use crate::error::{AlephError, Result};
use crate::utils::no_window::NoWindow;
use std::time::Duration;
use tokio::process::Command;
use tracing::{debug, error};

/// Shared oneshot execution logic used by both [`GenericAcpAdapter`] and
/// [`CustomAcpAdapter`].
pub(super) async fn run_oneshot_command(
    harness_id: &str,
    executable: &str,
    args: &[String],
    env: &[(String, String)],
    output_format: &OutputFormat,
    prompt: &str,
    cwd: &str,
    timeout: Duration,
) -> Result<String> {
    let mut cmd = Command::new(executable);
    cmd.args(args)
        .arg(prompt)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    for (key, val) in env {
        cmd.env(key, val);
    }

    debug!(
        harness = %harness_id,
        executable = %executable,
        "Spawning oneshot harness process"
    );

    let output = tokio::time::timeout(timeout, cmd.no_window().output())
        .await
        .map_err(|_| {
            AlephError::tool(format!(
                "Harness '{harness_id}' timed out after {timeout:?}"
            ))
        })?
        .map_err(|e| {
            AlephError::tool(format!(
                "Failed to execute harness '{harness_id}' (executable: '{executable}'): {e}. \
                 Is the executable installed and in PATH?"
            ))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        error!(
            harness = %harness_id,
            stderr = %stderr,
            "Harness process failed"
        );
        return Err(AlephError::tool(format!(
            "Harness '{}' exited with {}: {}",
            harness_id,
            output.status,
            stderr.chars().take(500).collect::<String>()
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(output_format.parse(&stdout))
}
