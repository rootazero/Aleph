//! Linux contract for an administrator-installed whisper.cpp v1.8.3 (`2eeeba56`).
//!
//! That release's CLI uses `/dev/stdout` for `-otxt -of -`, and `-np` disables
//! progress. No segment/timestamp log is parsed, and no transcript file is made.
//! The administrator must supply the supported, non-forking CLI (not a shell
//! wrapper) and its model. Startup checks path accessibility, not the version
//! or cryptographic identity, and does not pin either file for later use.

use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use aleph_desktop::media_types::SpeechToTextResult;
use aleph_desktop::{CancellationToken, DesktopError, Result};
use tokio::io::AsyncReadExt;
use tokio::process::{Child, ChildStdout, Command};

const RECOGNITION_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_TRANSCRIPT_BYTES: usize = 64 * 1024;

/// Validated local installation paths, supplied by the daemon at startup.
#[derive(Clone, Debug)]
pub struct OfflineSttInstallation {
    executable: PathBuf,
    model: PathBuf,
}

impl OfflineSttInstallation {
    /// Checks absolute regular-file paths, execute access as the daemon, and
    /// model readability. A later spawn can still fail, e.g. if a path changes.
    pub fn new(executable: PathBuf, model: PathBuf) -> Result<Self> {
        installed_file(&executable, "executable")?;
        if !is_executable_for_daemon(&executable) {
            return Err(DesktopError::NotAvailable(
                "whisper-cli installation is not executable".into(),
            ));
        }
        installed_file(&model, "model")?;
        fs::File::open(&model)
            .map_err(|_| DesktopError::NotAvailable("whisper-cli model is not readable".into()))?;
        Ok(Self { executable, model })
    }

    pub(crate) async fn transcribe(
        &self,
        audio_path: &str,
        language: Option<&str>,
        cancel: CancellationToken,
    ) -> Result<SpeechToTextResult> {
        self.transcribe_with_timeout(audio_path, language, cancel, RECOGNITION_TIMEOUT)
            .await
    }

    async fn transcribe_with_timeout(
        &self,
        audio_path: &str,
        language: Option<&str>,
        cancel: CancellationToken,
        timeout: Duration,
    ) -> Result<SpeechToTextResult> {
        let audio_path = PathBuf::from(audio_path);
        if !audio_path.is_absolute() {
            return Err(engine_failure("requires an absolute staged WAV path"));
        }
        let installation = self.clone();
        let language = language.unwrap_or("auto").to_owned();
        let caller_dropped = CancellationToken::new();
        let _cancel_on_drop = caller_dropped.clone().drop_guard();
        // The worker owns the child even if the awaiting future is dropped.
        // Explicit cancellation still awaits this same worker through reap.
        match tokio::spawn(async move {
            installation
                .recognize(audio_path, language, cancel, caller_dropped, timeout)
                .await
        })
        .await
        {
            Ok(result) => result,
            Err(error) => {
                tracing::error!(%error, "offline speech supervisor closed without release proof");
                std::future::pending().await
            }
        }
    }

    async fn recognize(
        &self,
        audio_path: PathBuf,
        language: String,
        cancel: CancellationToken,
        caller_dropped: CancellationToken,
        timeout: Duration,
    ) -> Result<SpeechToTextResult> {
        if cancel.is_cancelled() || caller_dropped.is_cancelled() {
            return Err(engine_failure("recognition cancelled"));
        }
        let deadline = tokio::time::Instant::now() + timeout;
        let mut child = Command::new(&self.executable)
            .arg("-m")
            .arg(&self.model)
            .arg("-f")
            .arg(audio_path)
            .arg("-l")
            .arg(&language)
            .args(["-np", "-nt", "-otxt", "-of", "-"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| {
                DesktopError::NotAvailable("could not start installed whisper-cli".into())
            })?;

        let result = if let Some(stdout) = child.stdout.take() {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => Err(engine_failure("recognition cancelled")),
                _ = caller_dropped.cancelled() => Err(engine_failure("recognition cancelled")),
                _ = tokio::time::sleep_until(deadline) => Err(engine_failure("recognition timed out")),
                output = async { tokio::try_join!(read_transcript(stdout), wait_for_success(&mut child)) } => {
                    // TXT mode does not report the detected language. Preserve
                    // the selection (including "auto") rather than guess it.
                    output.map(|(text, ())| SpeechToTextResult { text, language })
                }
            }
        } else {
            Err(engine_failure("stdout pipe unavailable"))
        };
        if result.is_err() {
            // The read future (and hence its pipe) has been dropped. A child
            // already reaped by wait_for_success makes start_kill a no-op.
            let _ = child.start_kill();
            if let Err(error) = child.wait().await {
                drop(child);
                // Returning would authorize Core to unlink the input. A failed
                // wait is not proof; retain the clip and permit as on macOS.
                tracing::error!(%error, "could not prove offline speech process was reaped");
                return std::future::pending().await;
            }
        }
        result
    }
}

fn is_executable_for_daemon(path: &Path) -> bool {
    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // Use Linux 5.8+ directly: older libc faccessat emulation can miss ACLs.
    // If the syscall is unavailable or blocked, do not mount the recognizer.
    // SAFETY: path is a live NUL-terminated buffer for the duration of the call.
    unsafe {
        libc::syscall(
            libc::SYS_faccessat2,
            libc::AT_FDCWD,
            path.as_ptr(),
            libc::X_OK,
            libc::AT_EACCESS,
        ) == 0
    }
}

fn installed_file(path: &Path, role: &str) -> Result<()> {
    if !path.is_absolute() {
        return Err(DesktopError::NotAvailable(format!(
            "whisper-cli {role} must use an absolute installation path"
        )));
    }
    fs::metadata(path)
        .ok()
        .filter(|metadata| metadata.is_file())
        .map(|_| ())
        .ok_or_else(|| {
            DesktopError::NotAvailable(format!("whisper-cli {role} is not an installed file"))
        })
}

async fn wait_for_success(child: &mut Child) -> Result<()> {
    let status = child
        .wait()
        .await
        .map_err(|_| engine_failure("could not wait for process"))?;
    if !status.success() {
        return Err(engine_failure("recognition process failed"));
    }
    Ok(())
}

async fn read_transcript(stdout: ChildStdout) -> Result<String> {
    let read_limit = MAX_TRANSCRIPT_BYTES + 1;
    let mut bytes = Vec::with_capacity(read_limit);
    stdout
        .take(read_limit as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| engine_failure("could not read transcript"))?;
    if bytes.len() > MAX_TRANSCRIPT_BYTES {
        return Err(engine_failure("transcript exceeds the byte limit"));
    }
    let text = String::from_utf8(bytes).map_err(|_| engine_failure("transcript is not UTF-8"))?;
    if text
        .chars()
        .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
    {
        return Err(engine_failure(
            "transcript contains invalid control characters",
        ));
    }
    // Core turns empty text into the normal NoSpeech response.
    Ok(text.trim().to_owned())
}

fn engine_failure(reason: &str) -> DesktopError {
    DesktopError::BridgeFailed(format!("whisper-cli {reason}"))
}

#[cfg(test)]
mod tests;
