//! Public browser recording DTOs.

use std::path::PathBuf;

/// The container a managed default path is generated for. When the caller
/// supplies an explicit path, the PATH's extension decides instead
/// (spec §3: 格式按扩展名).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingFormat {
    Webm,
    Mp4,
}

impl RecordingFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Webm => "webm",
            Self::Mp4 => "mp4",
        }
    }
}

/// What a finished recording reports. `complete` is a DERIVED verdict (see the
/// module doc), never a claim the encoder made about itself.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RecordingReceipt {
    pub recording_id: String,
    /// Absolute — the model never guesses where its file is (spec §3).
    pub path: PathBuf,
    pub duration_ms: u64,
    pub captured_frames: u64,
    pub encoded_frames: u64,
    pub dropped_frames: u64,
    /// The encoder's exit state, verbatim: `"0"`, `"1"`, `"signal 9"`,
    /// `"killed"` (we killed it), or `"wait error: …"`.
    pub encoder_exit: String,
    pub size_bytes: u64,
    pub complete: bool,
    pub truncation_reason: Option<String>,
}

/// A still-running recording, for `status`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RecordingStatus {
    pub recording_id: String,
    /// The resolved absolute output path (post-uniquify for managed
    /// defaults) — spec §4's `resolved_path`: the model never guesses where
    /// its file is going.
    pub path: PathBuf,
    pub elapsed_ms: u64,
    pub captured_frames: u64,
    pub dropped_frames: u64,
    /// Where the encoder binary came from (spec §4's `ffmpeg_source`) — the
    /// resolution chain's label (`ALEPH_FFMPEG env`, `ffmpeg on PATH`, the
    /// playwright bundle) or the explicit-injection seam's.
    pub ffmpeg_source: String,
}

/// Everything `start` needs beyond who/where to record. A struct rather than
/// a long positional list because `fps_cap` and `quality` are both bare
/// integers — adjacent, swappable, and invisible at the call site.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordStartOptions {
    /// Explicit output path. `None` → the managed default
    /// (default_recording_path).
    pub path: Option<PathBuf>,
    /// Shapes the container timestamps (`-framerate`); the screencast stream
    /// itself is repaint-driven, so this is an upper label, not a promise
    /// (spec §2). The receipt's `captured_frames / duration` is the truth.
    pub fps_cap: u32,
    /// Container for the MANAGED default path's extension. Ignored when
    /// `path` is given — then the path's own extension rules (spec §3).
    pub format: RecordingFormat,
    /// JPEG quality of the wire frames.
    pub quality: u8,
    /// Explicit encoder binary, skipping resolve_ffmpeg. The seam the
    /// pipeline tests inject stub encoders through — process-global env would
    /// race parallel tests. Excluded from wire encoding; strict decoding
    /// rejects an explicit `ffmpeg` key.
    #[serde(skip)]
    pub ffmpeg: Option<PathBuf>,
}

impl Default for RecordStartOptions {
    fn default() -> Self {
        Self {
            path: None,
            fps_cap: 10,
            format: RecordingFormat::Webm,
            quality: 80,
            ffmpeg: None,
        }
    }
}
