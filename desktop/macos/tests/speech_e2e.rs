//! End-to-end tests for the Swift-side speech-to-text handler.
//!
//! Requires **Speech Recognition + Microphone TCC permissions** on the dev
//! machine (System Settings → Privacy & Security → Speech Recognition and
//! Microphone → AlephBridge). The first run will trigger OS permission
//! prompts. Tests spawn the real compiled `AlephBridge` helper and are
//! `#[ignore]` by default; run with `just test-speech-e2e` (which builds the
//! helper first).

// Whole file is macOS-only: it drives the real `AlephBridge` helper via the
// macOS-only `aleph-desktop-macos` crate. On any other host the integration
// test compiles to an empty binary, which `cargo check --workspace` treats as
// "0 tests" — no failure.
#![cfg(target_os = "macos")]

use std::path::PathBuf;

use aleph_desktop::bridge::client::SwiftBridge;
use aleph_protocol::desktop_bridge::methods::media::{
    RecordAudioParams, RecordAudioResult, TranscribeFileParams, TranscribeFileResult,
    METHOD_AUDIO_RECORD, METHOD_SPEECH_TRANSCRIBE_FILE,
};

fn helper_path() -> PathBuf {
    // CARGO_MANIFEST_DIR for this crate is `desktop/macos`.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("bridge")
        .join(".build")
        .join("release")
        .join("AlephBridge")
}

/// Records a 2-second clip, then transcribes it through the Swift helper.
/// The clip may be silence depending on the environment; we only assert that
/// the RPC completes and echoes the requested language. Text is not asserted
/// non-empty because SFSpeechRecognizer can legitimately return an empty
/// string on pure silence.
#[tokio::test]
#[ignore]
async fn transcribe_known_audio_file_returns_text() {
    let path = helper_path();
    assert!(
        path.exists(),
        "helper not built at {}; run `just swift-bridge` first",
        path.display()
    );

    let bridge = SwiftBridge::new(path);

    // Step 1: record an audio file to feed the transcriber.
    let record: RecordAudioResult = bridge
        .call(
            METHOD_AUDIO_RECORD,
            RecordAudioParams { duration_secs: 2.0 },
        )
        .await
        .expect("media.audio.record RPC failed");
    assert!(!record.file_path.is_empty(), "file_path was empty");
    assert!(
        std::path::Path::new(&record.file_path).exists(),
        "audio file missing: {}",
        record.file_path
    );

    // Step 2: transcribe it.
    let result: TranscribeFileResult = bridge
        .call(
            METHOD_SPEECH_TRANSCRIBE_FILE,
            TranscribeFileParams {
                audio_path: record.file_path,
                language: "en-US".into(),
            },
        )
        .await
        .expect("media.speech.transcribe_file RPC failed");

    assert_eq!(result.language, "en-US", "language was not echoed");
    // Do NOT assert result.text non-empty; silent rooms legitimately yield "".
}
