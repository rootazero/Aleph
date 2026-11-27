use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
}

#[test]
fn offline_stt_windows_rejects_relative_worker_executable() {
    let error = runtime()
        .block_on(recognize(
            Path::new("aleph-server.exe"),
            "unused.wav",
            None,
            CancellationToken::new(),
        ))
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        platform("offline speech worker executable must be absolute").to_string()
    );
}

#[test]
fn offline_stt_windows_worker_protocol_is_strict_and_bounded() {
    let request = WorkerRequest {
        audio_path: PathBuf::from(r"C:\录音\clip.wav"),
        language: Some("zh-CN".into()),
    };
    let bytes = encode_record(&request, MAX_REQUEST_BYTES).unwrap();
    let decoded: WorkerRequest = decode_record(&bytes, MAX_REQUEST_BYTES).unwrap();
    assert_eq!(decoded.audio_path, request.audio_path);
    assert_eq!(decoded.language, request.language);
    assert!(decode_record::<WorkerRequest>(&bytes, bytes.len() - 1).is_none());

    for invalid in [
        &b"{\"status\":\"unsupported_language\"}"[..],
        b"{\"status\":\"unsupported_language\"}\n{}\n",
        b"{\"status\":\"unsupported_language\",\"text\":\"extra\"}\n",
        b"{\"status\":\"unsupported_language\",\"status\":\"no_default_recognizer\"}\n",
        b"{\"status\":\"success\",\"text\":\"\xff\",\"language\":\"zh-CN\"}\n",
        b"{\"status\":\"success\",\"text\":\"first\",\"text\":\"second\",\"language\":\"zh-CN\"}\n",
    ] {
        assert!(decode_record::<WorkerReply>(invalid, MAX_REPLY_BYTES).is_none());
    }
    for reply in [
        WorkerReply::Success {
            text: "é".repeat(MAX_TRANSCRIPT_BYTES / 2 + 1),
            language: "zh-CN".into(),
        },
        WorkerReply::Success {
            text: String::new(),
            language: "zh-TW".into(),
        },
        WorkerReply::NoDefaultRecognizer {},
    ] {
        assert!(interpret_reply(reply, &request).is_err());
    }
    assert!(
        matches!(interpret_reply(WorkerReply::UnsupportedLanguage {}, &request).unwrap(), Err(DesktopError::UnsupportedLanguage(language)) if language == "zh-CN")
    );
    let empty = interpret_reply(
        WorkerReply::Success {
            text: String::new(),
            language: "zh-CN".into(),
        },
        &request,
    )
    .unwrap()
    .unwrap();
    assert!(empty.text.is_empty());
    assert_eq!(empty.language, "zh-CN");
}

struct ProcessHandle(HANDLE);

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

struct Marker(PathBuf);

impl Marker {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "aleph-sapi-worker-{}-{}.ready",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_file(&path);
        Self(path)
    }

    async fn wait_until_entered(&self) -> ProcessHandle {
        // Wall-clock bound for fixture startup, independent of the paused test clock.
        let limit = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            if let Ok(text) = std::fs::read_to_string(&self.0) {
                if let Some(pid) = text.strip_suffix('\n') {
                    return ProcessHandle(unsafe {
                        OpenProcess(PROCESS_SYNCHRONIZE, false, pid.parse().unwrap()).unwrap()
                    });
                }
            }
            assert!(
                std::time::Instant::now() < limit,
                "blocked fixture did not handshake"
            );
            tokio::task::yield_now().await;
        }
    }
}

impl Drop for Marker {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(self.0.with_extension("go"));
    }
}

fn blocked_child(invalid_output: bool) -> Command {
    // Test-only OS fixture: read the real stdin record, signal entry with its
    // own PID, then block forever without polling cancellation.
    let mut directory = [0; 260];
    let len = unsafe { GetSystemDirectoryW(Some(&mut directory)) } as usize;
    assert!((1..directory.len()).contains(&len));
    let powershell = PathBuf::from(String::from_utf16(&directory[..len]).unwrap())
        .join("WindowsPowerShell/v1.0/powershell.exe");
    let mut command = Command::new(powershell);
    let mut script = String::from(
        r#"
        $ErrorActionPreference = 'Stop'
        $reader = New-Object IO.StreamReader([Console]::OpenStandardInput(), [Text.Encoding]::UTF8)
        $request = $reader.ReadToEnd() | ConvertFrom-Json
        [IO.File]::WriteAllText($request.audio_path, [string]$PID + [char]10)
    "#,
    );
    if invalid_output {
        script.push_str(
            r#"
            $go = [IO.Path]::ChangeExtension($request.audio_path, 'go')
            while (!(Test-Path -LiteralPath $go)) { [Threading.Thread]::Sleep(5) }
            [Console]::Out.WriteLine('invalid json')
            [Console]::Out.Flush()
        "#,
        );
    }
    script.push_str("[Threading.Thread]::Sleep(-1)");
    command.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
    command
}

#[test]
fn offline_stt_windows_blocked_child_is_reaped_before_return() {
    for (trigger, expected) in [
        ("deadline", deadline_exceeded()),
        ("cancel", cancelled()),
        ("invalid", invalid_reply()),
    ] {
        runtime().block_on(async {
            tokio::time::pause();
            let marker = Marker::new();
            let cancel = CancellationToken::new();
            let task = tokio::spawn(run_owned(
                blocked_child(trigger == "invalid"),
                WorkerRequest {
                    audio_path: marker.0.clone(),
                    language: None,
                },
                cancel.clone(),
                Instant::now() + RECOGNITION_TIMEOUT,
            ));
            let child = marker.wait_until_entered().await;
            assert_eq!(unsafe { WaitForSingleObject(child.0, 0) }, WAIT_TIMEOUT);
            match trigger {
                "deadline" => {
                    tokio::time::advance(RECOGNITION_TIMEOUT + Duration::from_millis(1)).await
                }
                "cancel" => cancel.cancel(),
                _ => std::fs::write(marker.0.with_extension("go"), []).unwrap(),
            }
            // Guard a broken cleanup path without auto-advancing its frozen deadline.
            let limit = std::time::Instant::now() + Duration::from_secs(15);
            while !task.is_finished() {
                assert!(
                    std::time::Instant::now() < limit,
                    "{trigger} did not complete"
                );
                tokio::task::yield_now().await;
            }
            assert_eq!(
                task.await.unwrap().unwrap_err().to_string(),
                expected.to_string()
            );
            assert_eq!(
                unsafe { WaitForSingleObject(child.0, 0) },
                WAIT_OBJECT_0,
                "{trigger} returned before exact child exit"
            );
        });
    }
}

#[test]
fn offline_stt_windows_dropping_caller_requests_exact_child_cleanup() {
    runtime().block_on(async {
        let marker = Marker::new();
        let cancel = CancellationToken::new();
        let task = tokio::spawn(run_owned(
            blocked_child(false),
            WorkerRequest {
                audio_path: marker.0.clone(),
                language: None,
            },
            cancel.clone(),
            Instant::now() + RECOGNITION_TIMEOUT,
        ));
        let child = marker.wait_until_entered().await;
        assert_eq!(unsafe { WaitForSingleObject(child.0, 0) }, WAIT_TIMEOUT);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(!cancel.is_cancelled());
        tokio::time::timeout(Duration::from_secs(15), async {
            while unsafe { WaitForSingleObject(child.0, 0) } == WAIT_TIMEOUT {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(unsafe { WaitForSingleObject(child.0, 0) }, WAIT_OBJECT_0);
    });
}
