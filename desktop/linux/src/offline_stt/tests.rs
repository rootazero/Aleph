use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use aleph_desktop::media_types::SpeechToTextConfig;
use aleph_desktop::DesktopPlatform;

use super::*;
use crate::LinuxPlatform;

struct Fixture {
    dir: tempfile::TempDir,
    executable: PathBuf,
    model: PathBuf,
    wav: PathBuf,
}

impl Fixture {
    fn new(script: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("whisper CLI;$(exit 1)");
        let model = dir.path().join("ggml model;$(exit 1).bin");
        let wav = dir.path().join("private audio;$(exit 1).wav");
        fs::write(&model, b"fixture model").unwrap();
        fs::write(&wav, b"fixture audio").unwrap();
        let mut file = fs::File::create(&executable).unwrap();
        writeln!(file, "#!/bin/sh\nset -eu\n{script}").unwrap();
        file.set_permissions(fs::Permissions::from_mode(0o700))
            .unwrap();
        Self {
            dir,
            executable,
            model,
            wav,
        }
    }

    fn installation(&self) -> OfflineSttInstallation {
        OfflineSttInstallation::new(self.executable.clone(), self.model.clone()).unwrap()
    }

    fn wav(&self) -> &str {
        self.wav.to_str().unwrap()
    }

    fn pid_path(&self) -> PathBuf {
        self.wav.with_extension("pid")
    }
}

#[tokio::test]
async fn offline_stt_linux_cli_is_literal_bounded_and_reaped() {
    let fixture = Fixture::new(
        r#"
        test -f "$2" && test -f "$4" || exit 9
        printf '%s\n' "$0" "$@" > "${4}.argv"
        printf '  bonjour, 世界  \n'
        printf 'progress on stderr\n' >&2
        "#,
    );
    let platform = LinuxPlatform::with_offline_stt(fixture.installation());
    let media = platform.media().unwrap();
    for (requested, expected) in [(Some("en;$(exit 1) -h"), "en;$(exit 1) -h"), (None, "auto")] {
        let result = media
            .speech_to_text_offline(fixture.wav(), requested, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(result.text, "bonjour, 世界");
        assert_eq!(result.language, expected);
        let argv = fs::read_to_string(format!("{}.argv", fixture.wav())).unwrap();
        assert_eq!(
            argv.lines().collect::<Vec<_>>(),
            [
                fixture.executable.to_str().unwrap(),
                "-m",
                fixture.model.to_str().unwrap(),
                "-f",
                fixture.wav(),
                "-l",
                expected,
                "-np",
                "-nt",
                "-otxt",
                "-of",
                "-",
            ]
        );
    }

    let fixture = Fixture::new(&format!("printf '%0{MAX_TRANSCRIPT_BYTES}d' 0"));
    let result = fixture
        .installation()
        .transcribe(fixture.wav(), None, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.text.len(), MAX_TRANSCRIPT_BYTES);

    let fixture = Fixture::new(&format!(
        "printf '%s\\n' \"$$\" > \"${{4%.*}}.pid\"; printf '%0{}d' 0; exec /bin/sleep 60",
        MAX_TRANSCRIPT_BYTES + 1
    ));
    let error = fixture
        .installation()
        .transcribe(fixture.wav(), None, CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "bridge failed: whisper-cli transcript exceeds the byte limit"
    );
    assert_reaped(pid_from(&fixture.pid_path()).unwrap());
}

#[tokio::test]
async fn offline_stt_linux_cli_rejects_malformed_or_failed_output() {
    for script in [
        "printf '\\377'",
        "printf 'text\\000more'",
        "printf 'valid-looking text'; exit 9",
    ] {
        let fixture = Fixture::new(script);
        assert!(matches!(
            fixture
                .installation()
                .transcribe(fixture.wav(), None, CancellationToken::new())
                .await,
            Err(DesktopError::BridgeFailed(_))
        ));
    }

    // Empty output reaches Core's existing NoSpeech normalization, not a transcript.
    for script in [":", "printf ' \\n\\t'"] {
        let fixture = Fixture::new(script);
        let result = fixture
            .installation()
            .transcribe(fixture.wav(), None, CancellationToken::new())
            .await
            .unwrap();
        assert!(result.text.is_empty());
    }
}

#[tokio::test]
async fn offline_stt_linux_is_opt_in_and_does_not_change_ordinary_stt() {
    let fixture = Fixture::new("printf 'offline transcript'");
    let default_platform = LinuxPlatform::new();
    assert!(matches!(
        default_platform
            .media()
            .unwrap()
            .speech_to_text_offline(fixture.wav(), None, CancellationToken::new())
            .await,
        Err(DesktopError::NotImplemented(_))
    ));
    let platform = LinuxPlatform::with_offline_stt(fixture.installation());
    assert!(matches!(
        platform
            .media()
            .unwrap()
            .speech_to_text(fixture.wav(), SpeechToTextConfig::default())
            .await,
        Err(DesktopError::NotImplemented(_))
    ));
    fs::remove_file(&fixture.executable).unwrap();
    assert!(matches!(
        platform
            .media()
            .unwrap()
            .speech_to_text_offline(fixture.wav(), None, CancellationToken::new())
            .await,
        Err(DesktopError::NotAvailable(_))
    ));
}

#[test]
fn offline_stt_linux_requires_absolute_installed_files() {
    let fixture = Fixture::new(":");
    assert!(OfflineSttInstallation::new("whisper-cli".into(), fixture.model.clone()).is_err());
    assert!(OfflineSttInstallation::new(fixture.executable.clone(), "ggml.bin".into()).is_err());
    assert!(OfflineSttInstallation::new(
        fixture.executable.clone(),
        fixture.wav.with_extension("missing")
    )
    .is_err());
    assert!(
        OfflineSttInstallation::new(fixture.executable.clone(), fixture.dir.path().to_owned())
            .is_err()
    );
    fs::set_permissions(&fixture.executable, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(OfflineSttInstallation::new(fixture.executable, fixture.model).is_err());
}

#[test]
fn offline_stt_linux_requires_execute_access_for_daemon() {
    let fixture = Fixture::new(":");
    let _installation = fixture.installation();
    fs::set_permissions(&fixture.executable, fs::Permissions::from_mode(0o601)).unwrap();

    // The owner can't use the other-execute bit, but a privileged daemon can.
    // Compare with actual execution rather than assuming what UID 0 can do.
    let spawn = std::process::Command::new(&fixture.executable).status();
    let installation = OfflineSttInstallation::new(fixture.executable, fixture.model);
    match spawn {
        Ok(status) => {
            assert!(status.success());
            assert!(installation.is_ok());
        }
        Err(error) => {
            assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
            assert!(matches!(
                installation,
                Err(DesktopError::NotAvailable(message))
                    if message == "whisper-cli installation is not executable"
            ));
        }
    }
}

// No descendants: exec preserves the signalled PID. Stderr and stdout otherwise
// stay open until the fixture is killed, just as a running recognizer would.
const RUNNING_CLI: &str = r#"printf '%s\n' "$$" > "${4%.*}.pid"; exec /bin/sleep 60"#;

fn pid_from(path: &Path) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

async fn wait_for_pid(path: &Path) -> u32 {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(pid) = pid_from(path) {
                return pid;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fixture never reported a PID")
}

fn proc_entry(pid: u32) -> PathBuf {
    Path::new("/proc").join(pid.to_string())
}

fn assert_reaped(pid: u32) {
    assert!(
        Path::new("/proc/self/status").exists(),
        "reap test requires procfs"
    );
    assert!(
        !proc_entry(pid).exists(),
        "fixture process (or zombie) {pid} leaked"
    );
}

#[tokio::test]
async fn offline_stt_linux_cancellation_waits_for_reap() {
    let fixture = Fixture::new(RUNNING_CLI);
    let installation = fixture.installation();
    let wav = fixture.wav().to_owned();
    let cancel = CancellationToken::new();
    let request = tokio::spawn({
        let cancel = cancel.clone();
        async move { installation.transcribe(&wav, None, cancel).await }
    });
    let pid = wait_for_pid(&fixture.pid_path()).await;
    assert!(proc_entry(pid).exists());
    cancel.cancel();
    let error = tokio::time::timeout(Duration::from_secs(5), request)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "bridge failed: whisper-cli recognition cancelled"
    );
    assert_reaped(pid);

    fs::remove_file(fixture.pid_path()).unwrap();
    assert!(fixture
        .installation()
        .transcribe(fixture.wav(), None, cancel)
        .await
        .is_err());
    assert!(
        !fixture.pid_path().exists(),
        "pre-cancelled request spawned the CLI"
    );
}

#[tokio::test]
async fn offline_stt_linux_timeout_waits_for_reap_after_stdout_eof() {
    let fixture = Fixture::new(&format!("exec 1>&-; {RUNNING_CLI}"));
    let error = fixture
        .installation()
        .transcribe_with_timeout(
            fixture.wav(),
            None,
            CancellationToken::new(),
            Duration::from_secs(2),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "bridge failed: whisper-cli recognition timed out"
    );
    assert_reaped(pid_from(&fixture.pid_path()).unwrap());
}

#[tokio::test]
async fn offline_stt_linux_dropped_request_still_reaps() {
    let fixture = Fixture::new(RUNNING_CLI);
    let installation = fixture.installation();
    let wav = fixture.wav().to_owned();
    let request = tokio::spawn(async move {
        installation
            .transcribe(&wav, None, CancellationToken::new())
            .await
    });
    let pid = wait_for_pid(&fixture.pid_path()).await;
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(5), async {
        while proc_entry(pid).exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("dropped request left the CLI running");
    assert_reaped(pid);
}
