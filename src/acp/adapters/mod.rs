//! Concrete ACP harness adapters for supported CLI tools.

mod custom;
mod generic;

pub use custom::CustomAcpAdapter;
pub use generic::GenericAcpAdapter;

use crate::acp::output_format::OutputFormat;
use crate::error::{AlephError, Result};
use crate::utils::no_window::NoWindow;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, error};

/// Maximum number of bytes captured from one oneshot harness stream.
///
/// `Command::output()` buffers the harness' whole stdout and stderr in memory:
/// a harness that prints a huge diff — or a runaway loop — would grow the
/// buffer until the OOM killer takes the whole server down, not just the one
/// session. The cap reuses the ACP filesystem read limit
/// (`incoming::MAX_FS_READ_BYTES`) so the module keeps a single "max payload
/// buffered per operation" number.
const MAX_ONESHOT_CAPTURE_BYTES: usize = crate::acp::incoming::MAX_FS_READ_BYTES as usize;

/// Chunk size of a single read from a harness pipe.
const CAPTURE_CHUNK_BYTES: usize = 8 * 1024;

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

    let deadline = tokio::time::Instant::now() + timeout;
    let mut child = cmd.no_window().spawn().map_err(|e| {
        AlephError::tool(format!(
            "Failed to execute harness '{harness_id}' (executable: '{executable}'): {e}. \
             Is the executable installed and in PATH?"
        ))
    })?;

    // Both pipes are drained concurrently: a harness that fills one pipe while
    // the other one is only being read would deadlock.
    let (overflow_tx, mut overflow_rx) = mpsc::channel::<()>(1);
    let stdout_task = child.stdout.take().map(|pipe| {
        tokio::spawn(capture_stream(
            pipe,
            MAX_ONESHOT_CAPTURE_BYTES,
            overflow_tx.clone(),
        ))
    });
    let stderr_task = child.stderr.take().map(|pipe| {
        tokio::spawn(capture_stream(
            pipe,
            MAX_ONESHOT_CAPTURE_BYTES,
            overflow_tx.clone(),
        ))
    });
    // Keep a sender alive for the whole run so `recv()` only completes on a
    // real overflow signal and never on a closed channel.
    let _overflow_guard = overflow_tx;

    let captured = tokio::select! {
        status = tokio::time::timeout_at(deadline, child.wait()) => {
            let status = status
                .map_err(|_| {
                    AlephError::tool(format!(
                        "Harness '{harness_id}' timed out after {timeout:?}"
                    ))
                })?
                .map_err(|e| {
                    AlephError::tool(format!("Harness '{harness_id}' wait failed: {e}"))
                })?;
            // The harness exited, so both pipes now reach EOF.
            let stdout = tokio::time::timeout_at(
                deadline,
                await_capture(stdout_task, harness_id, "stdout"),
            )
            .await
            .map_err(|_| {
                AlephError::tool(format!(
                    "Harness '{harness_id}' timed out after {timeout:?}"
                ))
            })??;
            let stderr = tokio::time::timeout_at(
                deadline,
                await_capture(stderr_task, harness_id, "stderr"),
            )
            .await
            .map_err(|_| {
                AlephError::tool(format!(
                    "Harness '{harness_id}' timed out after {timeout:?}"
                ))
            })??;
            (stdout, stderr, status)
        }
        _signal = overflow_rx.recv() => {
            // A stream exceeded the capture cap: stop the harness instead of
            // letting it grow the buffer without bound.
            if let Some(task) = stdout_task.as_ref() {
                task.abort();
            }
            if let Some(task) = stderr_task.as_ref() {
                task.abort();
            }
            match child.kill().await {
                Ok(()) => {}
                Err(err) => {
                    debug!(
                        harness = %harness_id,
                        error = %err,
                        "ACP adapter: failed to kill the harness after a capture overflow"
                    );
                }
            }
            return Err(AlephError::tool(format!(
                "Harness '{harness_id}' exceeded the {MAX_ONESHOT_CAPTURE_BYTES} byte output capture limit"
            )));
        }
    };
    let (stdout_bytes, stderr_bytes, status) = captured;

    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr_bytes);
        error!(
            harness = %harness_id,
            stderr = %stderr,
            "Harness process failed"
        );
        return Err(AlephError::tool(format!(
            "Harness '{}' exited with {}: {}",
            harness_id,
            status,
            stderr.chars().take(500).collect::<String>()
        )));
    }

    let stdout = String::from_utf8_lossy(&stdout_bytes);
    Ok(output_format.parse(&stdout))
}

/// Read one harness pipe into memory, stopping at `limit` bytes.
///
/// Bytes past the limit are never buffered: the reader stops and reports the
/// overflow so the caller can kill the harness instead of growing the buffer
/// without bound.
async fn capture_stream<R>(
    mut reader: R,
    limit: usize,
    overflow_tx: mpsc::Sender<()>,
) -> std::io::Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let mut buf = Vec::new();
    let mut chunk = [0u8; CAPTURE_CHUNK_BYTES];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        if buf.len() + read > limit {
            // Report the overflow once; the caller kills the harness.
            if overflow_tx.send(()).await.is_err() {
                debug!("ACP adapter: overflow signal receiver already closed");
            }
            break;
        }
        buf.extend_from_slice(&chunk[..read]);
    }
    Ok(buf)
}

/// Await one capture reader and surface its failure instead of discarding it.
async fn await_capture(
    task: Option<JoinHandle<std::io::Result<Vec<u8>>>>,
    harness_id: &str,
    stream: &str,
) -> Result<Vec<u8>> {
    let Some(handle) = task else {
        return Err(AlephError::tool(format!(
            "Harness '{harness_id}' produced no {stream} pipe"
        )));
    };
    handle
        .await
        .map_err(|e| {
            AlephError::tool(format!(
                "Harness '{harness_id}' {stream} reader failed: {e}"
            ))
        })?
        .map_err(|e| {
            AlephError::tool(format!(
                "Harness '{harness_id}' {stream} capture failed: {e}"
            ))
        })
}
