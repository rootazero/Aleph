//! Supervisor and private process protocol for Windows offline dictation.

mod sapi;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use aleph_desktop::media_types::SpeechToTextResult;
use aleph_desktop::{CancellationToken, DesktopError, Result};
use aleph_protocol::desktop_bridge::methods::media::OFFLINE_SPEECH_DEADLINE_MS;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::Instant;
use windows::Win32::System::Threading::CREATE_NO_WINDOW;

/// Private early-dispatch argument for the same trusted server executable.
#[doc(hidden)]
pub const OFFLINE_STT_WORKER_ARG: &str = "--aleph-private-offline-stt-worker";

const RECOGNITION_TIMEOUT: Duration = Duration::from_millis(OFFLINE_SPEECH_DEADLINE_MS);
const MAX_PATH_UTF16: usize = 32_767;
const MAX_LOCALE_UTF16: usize = 84;
const MAX_TRANSCRIPT_BYTES: usize = 16 * 1024;
// Include JSON escaping, field names, and the terminating LF in the wire caps.
const MAX_REQUEST_BYTES: usize = 6 * (MAX_PATH_UTF16 + MAX_LOCALE_UTF16) + 128;
const MAX_REPLY_BYTES: usize = 6 * (MAX_TRANSCRIPT_BYTES + MAX_LOCALE_UTF16) + 128;
const MAX_ERROR_BYTES: usize = 256;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WorkerRequest {
    audio_path: PathBuf,
    language: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum WorkerReply {
    Success { text: String, language: String },
    UnsupportedLanguage {},
    NoDefaultRecognizer {},
    PlatformError { message: String },
}

/// Run the private Windows SAPI worker using one bounded stdin/stdout record.
/// Dispatch before server startup; call only for `OFFLINE_STT_WORKER_ARG`.
/// The return value is the process exit code, not the recognition result.
#[doc(hidden)]
pub fn run_offline_stt_worker() -> i32 {
    let deadline = std::time::Instant::now() + RECOGNITION_TIMEOUT;
    let mut input = Vec::new();
    let reply = (|| {
        std::io::stdin()
            .take(MAX_REQUEST_BYTES as u64 + 1)
            .read_to_end(&mut input)
            .ok()?;
        let request: WorkerRequest = decode_record(&input, MAX_REQUEST_BYTES)?;
        validate_request(&request).ok()?;
        let reply =
            match sapi::recognize(&request.audio_path, request.language.as_deref(), deadline) {
                Ok(SpeechToTextResult { text, language }) => {
                    WorkerReply::Success { text, language }
                }
                Err(DesktopError::UnsupportedLanguage(_)) => WorkerReply::UnsupportedLanguage {},
                Err(DesktopError::NotImplemented(_)) => WorkerReply::NoDefaultRecognizer {},
                Err(DesktopError::PlatformError(message)) => WorkerReply::PlatformError { message },
                Err(_) => return None,
            };
        encode_record(&reply, MAX_REPLY_BYTES)
    })();
    match reply {
        Some(record) => {
            let mut out = std::io::stdout().lock();
            if out.write_all(&record).and_then(|()| out.flush()).is_ok() {
                0
            } else {
                1
            }
        }
        None => 1,
    }
}

pub(super) async fn recognize(
    server_exe: &Path,
    audio_path: &str,
    language: Option<&str>,
    cancel: CancellationToken,
) -> Result<SpeechToTextResult> {
    if !server_exe.is_absolute() {
        return Err(platform(
            "offline speech worker executable must be absolute",
        ));
    }
    let deadline = Instant::now() + RECOGNITION_TIMEOUT;
    let request = WorkerRequest {
        audio_path: PathBuf::from(audio_path),
        language: language.map(str::to_owned),
    };
    let mut command = Command::new(server_exe);
    command.arg(OFFLINE_STT_WORKER_ARG);
    run_owned(command, request, cancel, deadline).await
}

async fn run_owned(
    command: Command,
    request: WorkerRequest,
    cancel: CancellationToken,
    deadline: Instant,
) -> Result<SpeechToTextResult> {
    if cancel.is_cancelled() {
        return Err(cancelled());
    }
    validate_request(&request)?;
    let record = encode_record(&request, MAX_REQUEST_BYTES)
        .ok_or_else(|| platform("invalid offline speech worker request"))?;
    let worker_cancel = cancel.child_token();
    // Dropping the caller signals the owning task rather than blocking a Tokio
    // thread. Explicit cancellation awaits that task's process-exit proof.
    let _on_drop = worker_cancel.clone().drop_guard();
    match tokio::spawn(supervise(command, request, record, worker_cancel, deadline)).await {
        Ok(result) => result,
        // Dispatch may have started; no normal return is safe without proof.
        Err(_) => unknown_process_exit().await,
    }
}

async fn supervise(
    mut command: Command,
    request: WorkerRequest,
    record: Vec<u8>,
    cancel: CancellationToken,
    deadline: Instant,
) -> Result<SpeechToTextResult> {
    if cancel.is_cancelled() {
        return Err(cancelled());
    }
    if Instant::now() >= deadline {
        return Err(deadline_exceeded());
    }
    let mut child = command
        .creation_flags(CREATE_NO_WINDOW.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| platform("could not start offline speech worker"))?;
    let result = tokio::select! {
        biased;
        () = cancel.cancelled() => Err(cancelled()),
        () = tokio::time::sleep_until(deadline) => Err(deadline_exceeded()),
        result = exchange(&mut child, &request, &record) => result,
    };
    match result {
        Ok(reply) => reply,
        Err(error) => {
            // Waiting successfully is required even if kill reports an error
            // (for example, if the process exited just before termination).
            let _ = child.start_kill();
            if child.wait().await.is_err() {
                unknown_process_exit().await
            } else {
                Err(error)
            }
        }
    }
}

// The outer result is for failures of the exchange (terminate and reap); the
// inner result is the worker's typed answer after a confirmed clean exit.
async fn exchange(
    child: &mut Child,
    request: &WorkerRequest,
    record: &[u8],
) -> Result<Result<SpeechToTextResult>> {
    let mut stdin = child.stdin.take().ok_or_else(invalid_reply)?;
    stdin.write_all(record).await.map_err(|_| invalid_reply())?;
    stdin.flush().await.map_err(|_| invalid_reply())?;
    // EOF delimits the worker's only request.
    drop(stdin);

    let mut stdout = BufReader::new(child.stdout.take().ok_or_else(invalid_reply)?)
        .take(MAX_REPLY_BYTES as u64 + 1);
    let mut bytes = Vec::new();
    stdout
        .read_until(b'\n', &mut bytes)
        .await
        .map_err(|_| invalid_reply())?;
    let reply: WorkerReply = decode_record(&bytes, MAX_REPLY_BYTES).ok_or_else(invalid_reply)?;
    let result = interpret_reply(reply, request)?;
    let mut trailing = [0];
    if stdout
        .read(&mut trailing)
        .await
        .map_err(|_| invalid_reply())?
        != 0
    {
        return Err(invalid_reply());
    }
    if !child.wait().await.map_err(|_| invalid_reply())?.success() {
        return Err(invalid_reply());
    }
    Ok(result)
}

fn interpret_reply(
    reply: WorkerReply,
    request: &WorkerRequest,
) -> Result<Result<SpeechToTextResult>> {
    Ok(match reply {
        WorkerReply::Success { text, language }
            if text.len() <= MAX_TRANSCRIPT_BYTES
                && !language.is_empty()
                && !language.contains('\0')
                && language.encode_utf16().count() <= MAX_LOCALE_UTF16
                && request
                    .language
                    .as_ref()
                    .is_none_or(|wanted| wanted == &language) =>
        {
            Ok(SpeechToTextResult { text, language })
        }
        WorkerReply::UnsupportedLanguage {} => Err(DesktopError::UnsupportedLanguage(
            request.language.clone().ok_or_else(invalid_reply)?,
        )),
        WorkerReply::NoDefaultRecognizer {} if request.language.is_none() => {
            Err(no_default_token())
        }
        WorkerReply::PlatformError { message }
            if !message.is_empty() && message.len() <= MAX_ERROR_BYTES =>
        {
            Err(platform(message))
        }
        _ => return Err(invalid_reply()),
    })
}

fn validate_request(request: &WorkerRequest) -> Result<()> {
    if let Some(language) = &request.language {
        if language.is_empty()
            || language.encode_utf16().take(MAX_LOCALE_UTF16 + 1).count() > MAX_LOCALE_UTF16
            || language.contains('\0')
        {
            return Err(DesktopError::UnsupportedLanguage(language.clone()));
        }
    }
    let path = request
        .audio_path
        .as_os_str()
        .to_str()
        .ok_or_else(invalid_reply)?;
    if path.is_empty()
        || path.encode_utf16().take(MAX_PATH_UTF16 + 1).count() > MAX_PATH_UTF16
        || path.contains('\0')
    {
        return Err(platform("invalid offline speech input path"));
    }
    Ok(())
}

fn encode_record(value: &impl Serialize, max_bytes: usize) -> Option<Vec<u8>> {
    let mut bytes = serde_json::to_vec(value).ok()?;
    if bytes.len() >= max_bytes {
        return None;
    }
    bytes.push(b'\n');
    Some(bytes)
}

fn decode_record<T: serde::de::DeserializeOwned>(bytes: &[u8], max_bytes: usize) -> Option<T> {
    if bytes.len() > max_bytes || bytes.last() != Some(&b'\n') {
        return None;
    }
    let value = &bytes[..bytes.len() - 1];
    if value.contains(&b'\n') {
        return None;
    }
    serde_json::from_str(std::str::from_utf8(value).ok()?).ok()
}

async fn unknown_process_exit<T>() -> Result<T> {
    // Offline media's return is a release acknowledgement for the input file.
    // Keep it pending when the process lifetime cannot be proven.
    std::future::pending().await
}

fn no_default_token() -> DesktopError {
    DesktopError::NotImplemented("no installed default SAPI recognizer".into())
}

fn invalid_reply() -> DesktopError {
    platform("invalid offline speech worker response")
}

fn cancelled() -> DesktopError {
    platform("SAPI offline speech cancelled")
}

fn deadline_exceeded() -> DesktopError {
    platform("SAPI offline speech deadline exceeded")
}

fn platform(message: impl Into<String>) -> DesktopError {
    DesktopError::PlatformError(message.into())
}

#[cfg(test)]
mod supervisor_tests;
