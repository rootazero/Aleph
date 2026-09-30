//! Session recording: CDP `Page.startScreencast` frames → ffmpeg `image2pipe`
//! → a verified file on disk (spec §2/§3).
//!
//! The registry outlives any `CdpBackend` (backends are rebuilt per call), so
//! it lives on `ProfileManager` next to `RouteRegistry` and the backend carries
//! an `Arc` — the residency argument routes.rs:1-4 makes.
//!
//! Two invariants this module exists to keep honest:
//!
//! - **ack pacing** (Review Focus #1): a frame is acked only AFTER its bytes
//!   reached the encoder's stdin (or after it was deliberately dropped — a
//!   drop is also "dealt with", and an un-acked frame stops the engine's
//!   stream). Ack-before-write would let a slow encoder turn into unbounded
//!   in-memory buffering on a 16GB machine.
//! - **verified stop** (Review Focus #3): `complete = true` requires ALL of
//!   encoder exit code 0, file exists, size > 0, and an mtime inside the
//!   recording window. "The file is there" is never enough.

use std::path::{Path, PathBuf};

/// Where managed recordings live: `<aleph home>/recordings/<profile>/…`.
const MANAGED_DIR: &str = "recordings";

/// Refuse to start a recording when the target filesystem has less than this
/// much space left (spec §3: <500MB 拒录).
const MIN_DISK_BYTES: u64 = 500 * 1024 * 1024;

/// Why a recording could not start. Every variant is a fail-fast BEFORE any
/// encoder process or screencast stream exists — a start that fails leaves no
/// half-file and no registry residue.
#[derive(Debug, thiserror::Error)]
pub enum RecordStartError {
    #[error("tab already has an active recording: {existing_id}")]
    AlreadyRecording { existing_id: String },
    #[error(
        "no ffmpeg found; searched: {}. Install ffmpeg, set ALEPH_FFMPEG, or install the \
         playwright bundle.",
        searched.join(", ")
    )]
    NoFfmpeg { searched: Vec<String> },
    #[error("output path is not writable: {}", path.display())]
    PathNotWritable { path: PathBuf },
    /// The extension chooses the codec (spec §3), so a path without one is a
    /// caller error, not something to guess at.
    #[error("output path has no extension: {}", path.display())]
    NoExtension { path: PathBuf },
    #[error("disk space below the 500MB floor: {available_bytes} bytes available")]
    DiskLow { available_bytes: u64 },
    /// The engine refused mid-sequence (e.g. `Page.startScreencast`).
    #[error(transparent)]
    Engine(#[from] aleph_cdp::CdpError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// The container a managed default path is generated for. When the caller
/// supplies an explicit path, the PATH's extension decides instead
/// (spec §3: 格式按扩展名).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

/// The ffmpeg candidate list, in priority order, each with the label the
/// `NoFfmpeg` error reports. `None` means "this source does not have one".
fn ffmpeg_candidates() -> Vec<(String, Option<PathBuf>)> {
    let env = std::env::var_os("ALEPH_FFMPEG")
        .map(PathBuf::from)
        .filter(|p| p.is_file());
    let on_path = which_on_path("ffmpeg");
    let playwright = playwright_ffmpeg().filter(|p| p.is_file());
    vec![
        ("ALEPH_FFMPEG env".to_string(), env),
        ("ffmpeg on PATH".to_string(), on_path),
        (
            "playwright bundled ffmpeg (ms-playwright/ffmpeg-1011)".to_string(),
            playwright,
        ),
    ]
}

/// First `Some` in priority order, WITH its source label — the single
/// decision rule for [`resolve_ffmpeg`], extracted so the priority is
/// testable without mutating process-global env (which would race the
/// pipeline tests). The label rides along because spec §4's start answer
/// names WHERE the encoder came from (`ffmpeg_source`), and a label
/// reconstructed downstream would be a second guess at this one decision.
fn pick_ffmpeg(candidates: &[(String, Option<PathBuf>)]) -> Option<(String, PathBuf)> {
    candidates
        .iter()
        .find_map(|(label, p)| p.clone().map(|p| (label.clone(), p)))
}

/// The encoder binary and where it came from: `ALEPH_FFMPEG` → PATH → the
/// playwright bundle (spec §3 的解析链，§4 的 `ffmpeg_source`).
pub(crate) fn resolve_ffmpeg_labeled() -> Option<(PathBuf, String)> {
    pick_ffmpeg(&ffmpeg_candidates()).map(|(label, p)| (p, label))
}

/// `which(1)` by hand: the first executable named `name` on PATH. (The `which`
/// crate is not a dependency and pulling one in for a PATH scan would be R3.)
fn which_on_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(name))
            .find(|p| is_executable(p))
    })
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.is_file()
        && std::fs::metadata(p)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

/// The playwright bundle's own ffmpeg (chromium_resolve.rs:599-610 names the
/// `ffmpeg-1011` revision; the cache root differs per platform).
fn playwright_ffmpeg() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    #[cfg(target_os = "linux")]
    let p = home.join(".cache/ms-playwright/ffmpeg-1011/ffmpeg-linux");
    #[cfg(target_os = "macos")]
    let p = home.join("Library/Caches/ms-playwright/ffmpeg-1011/ffmpeg-mac");
    #[cfg(target_os = "windows")]
    let p = home.join("AppData/Local/ms-playwright/ffmpeg-1011/ffmpeg-win64.exe");
    Some(p)
}

/// Characters that are safe in a filename component of every supported
/// filesystem; everything else becomes `_`. Engine-chosen tab ids are
/// arbitrary strings, and a `/` must never become a directory.
fn sanitize_component(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// The managed default: `~/.aleph/recordings/<profile>/<utc>-<tab_id>.<ext>`
/// (spec §3). Pure in its inputs; the Aleph home follows the one authoritative
/// rule (`ALEPH_HOME` override included), and if no home exists at all the
/// fallback is the temp dir rather than a panic — a machine with no HOME has
/// bigger problems, and a recording that lands in tmp says so in its receipt.
pub(crate) fn default_recording_path(profile: &str, tab_id: &str, ext: &str) -> PathBuf {
    let home = crate::utils::paths::get_config_dir()
        .unwrap_or_else(|_| std::env::temp_dir().join("aleph"));
    let utc = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    home.join(MANAGED_DIR)
        .join(sanitize_component(profile))
        .join(format!("{utc}-{}.{ext}", sanitize_component(tab_id)))
}

/// Move aside for a real collision (`a.webm` → `a-2.webm`, `a-3.webm`, …) so a
/// second recording started within the same UTC second never clobbers the
/// first one's file. Existence is checked at START; host-owned files are never
/// overwritten silently (spec §3: 永不自动删, and never silently replaced).
fn uniquify(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    for n in 2..100 {
        let candidate = parent.join(format!("{stem}-{n}.{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    path
}

/// The codec arguments for the output path's extension (spec §3 的格式映射).
/// `webm` → VP8 — note the encoder's actual name in ffmpeg is `libvpx`;
/// `libvpx-vp8` does not exist (checked against ffmpeg n9.0.1's encoder list).
/// `mp4` → `libx264`. Any other PRESENT extension is passed through with no
/// codec hint, leaving the guess to ffmpeg, which reports its verdict as a
/// nonzero exit in the stop receipt.
fn codec_args_for(path: &Path) -> Result<Vec<String>, RecordStartError> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        None => Err(RecordStartError::NoExtension {
            path: path.to_path_buf(),
        }),
        Some("webm") => Ok(vec!["-c:v".to_string(), "libvpx".to_string()]),
        Some("mp4") => Ok(vec!["-c:v".to_string(), "libx264".to_string()]),
        Some(_) => Ok(Vec::new()),
    }
}

/// Writability is tested by DOING (create-write-delete a probe file), never by
/// guessing from permission bits: root writes to /proc and read-only mounts
/// laugh at mode bits (Review Focus #4).
///
/// The probe name must NOT contain the substring ".aleph":
/// `utils::paths::tests::no_hand_rolled_aleph_home_outside_the_allowlist`
/// scans at FILE level for `dirs::home_dir()` + ".aleph" together, and this
/// file legitimately has the former (`playwright_ffmpeg` resolves the
/// ms-playwright cache, a real-HOME path that ALEPH_HOME redirection must NOT
/// rewrite). The only Aleph-rooted path here — `default_recording_path` —
/// already goes through `get_config_dir()`, so the file is genuinely clean
/// and the name stays out of the guard's way.
fn probe_writable(dir: &Path) -> std::io::Result<()> {
    let probe = dir.join(format!(
        ".rec-probe-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&probe, b"probe")?;
    std::fs::remove_file(&probe)?;
    Ok(())
}

// =============================================================================
// The pipeline: frames → bounded channel → ffmpeg stdin, with ack pacing
// =============================================================================

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant, SystemTime};

use aleph_cdp::methods::screencast::{self, ScreencastFormat};
use aleph_cdp::{CdpConnection, EventStream, SessionId};
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, watch};

use crate::browser::error::BrowserError;

/// Frames buffered between the event loop and the encoder's stdin. 32 is a
/// few seconds of screencast at the default cap; past it the encoder is
/// genuinely behind and frames are DROPPED (counted, acked) rather than
/// buffered — the 16GB-memory clause of Review Focus #1.
const CHANNEL_CAPACITY: usize = 32;

/// What a finished recording reports. `complete` is a DERIVED verdict (see the
/// module doc), never a claim the encoder made about itself.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
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
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
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
#[derive(Clone, Debug)]
pub struct RecordStartOptions {
    /// Explicit output path. `None` → the managed default
    /// ([`default_recording_path`]).
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
    /// Explicit encoder binary, skipping [`resolve_ffmpeg`]. The seam the
    /// pipeline tests inject stub encoders through — process-global env would
    /// race parallel tests.
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

#[derive(Default)]
struct Counters {
    captured: AtomicU64,
    encoded: AtomicU64,
    dropped: AtomicU64,
}

/// The receipt a supervisor mints exactly once at the end of a recording.
/// Delivery is two-lane: a `stop()` that took the live entry reads it here
/// after joining the task; a recording that ended on its own (tab death,
/// encoder death) has it filed into the registry's `finished` map instead.
#[derive(Default)]
struct ReceiptSlot {
    receipt: Mutex<Option<RecordingReceipt>>,
}

struct ActiveRecording {
    id: String,
    /// The resolved output path, kept so `status()` can report where the
    /// frames are landing while the recording is still in flight.
    path: PathBuf,
    /// The encoder source label, kept so `status()` reports the same
    /// `ffmpeg_source` the start answer did.
    ffmpeg_source: String,
    started: Instant,
    counters: Arc<Counters>,
    stop_tx: watch::Sender<bool>,
    join: Mutex<Option<tokio::task::JoinHandle<()>>>,
    slot: Arc<ReceiptSlot>,
    /// Kept so `stop` can send `Page.stopScreencast` itself (best-effort —
    /// the tab may already be dead, which is often WHY stop is called).
    conn: CdpConnection,
    session: SessionId,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// One active recording per `(profile, tab_id)`; tab ids are engine-chosen
/// strings, so the profile rides in the key (routes.rs:102-106 同款理由).
/// Owned by `ProfileManager`; every `CdpBackend` carries an `Arc`.
pub struct RecordingRegistry {
    live: Mutex<HashMap<(String, String), Arc<ActiveRecording>>>,
    /// Receipts of recordings that ended WITHOUT a stop (tab/engine death,
    /// encoder death), waiting for the `stop` that collects them. Taken on
    /// read: a stop is terminal.
    finished: Mutex<HashMap<(String, String), RecordingReceipt>>,
    /// `rec1, rec2…` monotonic, never reused — the model quotes ids back.
    counter: AtomicU64,
    /// Serialises start's check→reserve→spawn so two concurrent starts of one
    /// tab cannot both pass the emptiness check (routes.rs 同款).
    start_lock: tokio::sync::Mutex<()>,
    /// The stop-path patience, spent at most twice (writer drain, then encoder
    /// wait). Set from the profile's CDP command timeout at construction.
    finalize_budget: Duration,
}

impl RecordingRegistry {
    #[must_use]
    pub fn new(finalize_budget: Duration) -> Self {
        Self {
            live: Mutex::new(HashMap::new()),
            finished: Mutex::new(HashMap::new()),
            counter: AtomicU64::new(0),
            start_lock: tokio::sync::Mutex::new(()),
            finalize_budget,
        }
    }

    /// Start recording `tab_id`. The order is the plan's seven steps and every
    /// step fails fast without residue: ffmpeg resolution → path extension +
    /// measured writability → disk floor → event subscription (BEFORE the
    /// cause) → `Page.startScreencast` → encoder spawn (rolled back via
    /// `stopScreencast` on failure) → registration + supervisor task.
    pub async fn start(
        self: &Arc<Self>,
        profile: &str,
        tab_id: &str,
        session: &SessionId,
        conn: &CdpConnection,
        options: RecordStartOptions,
    ) -> Result<RecordingStatus, RecordStartError> {
        let _serial = self.start_lock.lock().await;
        let key = (profile.to_string(), tab_id.to_string());
        if let Some(existing) = lock(&self.live).get(&key) {
            return Err(RecordStartError::AlreadyRecording {
                existing_id: existing.id.clone(),
            });
        }

        // 1. The encoder binary, with its source label kept (spec §4's
        // `ffmpeg_source` — the receipt of WHERE the encoder came from, so a
        // surprise codec build is diagnosable from the start answer alone).
        let (ffmpeg, ffmpeg_source) = match options.ffmpeg.clone() {
            Some(explicit) => (
                explicit,
                "RecordStartOptions::ffmpeg (explicit, not the resolution chain)".to_string(),
            ),
            None => resolve_ffmpeg_labeled().ok_or_else(|| RecordStartError::NoFfmpeg {
                searched: ffmpeg_candidates()
                    .into_iter()
                    .map(|(label, _)| label)
                    .collect(),
            })?,
        };

        // 2. The output path: absolutise (relative paths anchor at the
        // process cwd, spec §3), check the extension, and MEASURE
        // writability. uniquify is for managed defaults only — an explicit
        // path is the name the caller chose, and renaming it would make the
        // receipt report a file the caller never asked for.
        let managed = options.path.is_none();
        let raw = options
            .path
            .clone()
            .unwrap_or_else(|| default_recording_path(profile, tab_id, options.format.extension()));
        let raw = if raw.is_absolute() {
            raw
        } else {
            std::env::current_dir()?.join(raw)
        };
        if managed {
            if let Some(parent) = raw.parent() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let path = if managed { uniquify(raw) } else { raw };
        let codec_args = codec_args_for(&path)?;
        let parent = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        probe_writable(&parent)
            .map_err(|_| RecordStartError::PathNotWritable { path: path.clone() })?;

        // 3. The disk floor (spec §3: <500MB 拒录).
        if let Ok(available) = fs2::available_space(&parent) {
            if available < MIN_DISK_BYTES {
                return Err(RecordStartError::DiskLow {
                    available_bytes: available,
                });
            }
        }

        // 4. Subscribe BEFORE the cause: a broadcast starts at subscription,
        // so subscribing after `startScreencast` could miss the first frames
        // (events.rs module doc; C1 InterceptLoop 同款纪律).
        let events = conn.events();

        // 5. Start the frame stream.
        let wire = ScreencastFormat::Jpeg {
            quality: options.quality,
        };
        screencast::start_screencast(conn, Some(session), wire, None, None, None).await?;

        // 6. Spawn the encoder; a failure here rolls the stream back so the
        // engine is not left streaming into nothing.
        let child = match spawn_ffmpeg(&ffmpeg, &path, options.fps_cap, wire, &codec_args) {
            Ok(child) => child,
            Err(e) => {
                let _ = screencast::stop_screencast(conn, Some(session)).await;
                return Err(e.into());
            }
        };

        // 7. Register, then supervise. The entry exists in the map BEFORE the
        // task starts so a stop racing the first frame still finds it.
        let id = format!("rec{}", self.counter.fetch_add(1, Ordering::Relaxed) + 1);
        let counters = Arc::new(Counters::default());
        let (stop_tx, stop_rx) = watch::channel(false);
        let slot = Arc::new(ReceiptSlot::default());
        let entry = Arc::new(ActiveRecording {
            id: id.clone(),
            path: path.clone(),
            ffmpeg_source: ffmpeg_source.clone(),
            started: Instant::now(),
            counters: counters.clone(),
            stop_tx,
            join: Mutex::new(None),
            slot: slot.clone(),
            conn: conn.clone(),
            session: session.clone(),
        });
        lock(&self.live).insert(key.clone(), entry.clone());
        let join = tokio::spawn(run_recording(RunParams {
            conn: conn.clone(),
            session: session.clone(),
            events,
            child,
            counters,
            stop_rx,
            slot,
            path,
            window_start: SystemTime::now(),
            budget: self.finalize_budget,
            registry: Arc::downgrade(self),
            key,
            recording_id: id.clone(),
            started: entry.started,
        }));
        *lock(&entry.join) = Some(join);
        Ok(RecordingStatus {
            recording_id: id,
            path: entry.path.clone(),
            elapsed_ms: 0,
            captured_frames: 0,
            dropped_frames: 0,
            ffmpeg_source: entry.ffmpeg_source.clone(),
        })
    }

    /// Stop the recording of `tab_id`: `Page.stopScreencast` (best-effort),
    /// then the verified finalize. If the recording already ended on its own,
    /// the receipt it filed is what you get — a truncation is reported as a
    /// truncation either way.
    pub async fn stop(
        &self,
        profile: &str,
        tab_id: &str,
    ) -> Result<RecordingReceipt, BrowserError> {
        let key = (profile.to_string(), tab_id.to_string());
        let entry = lock(&self.live).remove(&key);
        match entry {
            Some(entry) => self.finalize_via_entry(entry).await,
            None => lock(&self.finished)
                .remove(&key)
                .ok_or_else(|| BrowserError::NoActiveRecording(tab_id.to_string())),
        }
    }

    /// The `recording_id` entrance of stop (spec §4: stop takes either).
    pub async fn stop_by_id(&self, recording_id: &str) -> Result<RecordingReceipt, BrowserError> {
        let key = lock(&self.live)
            .iter()
            .find(|(_, e)| e.id == recording_id)
            .map(|(k, _)| k.clone());
        if let Some((profile, tab_id)) = key {
            return self.stop(&profile, &tab_id).await;
        }
        let finished_key = lock(&self.finished)
            .iter()
            .find(|(_, r)| r.recording_id == recording_id)
            .map(|(k, _)| k.clone());
        if let Some(k) = finished_key {
            if let Some(receipt) = lock(&self.finished).remove(&k) {
                return Ok(receipt);
            }
        }
        Err(BrowserError::NoActiveRecording(recording_id.to_string()))
    }

    /// The live view, or `None` — including when a finished-but-uncollected
    /// receipt exists, because that recording is NOT in progress.
    pub fn status(&self, profile: &str, tab_id: &str) -> Option<RecordingStatus> {
        lock(&self.live)
            .get(&(profile.to_string(), tab_id.to_string()))
            .map(|e| RecordingStatus {
                recording_id: e.id.clone(),
                path: e.path.clone(),
                elapsed_ms: e.started.elapsed().as_millis() as u64,
                captured_frames: e.counters.captured.load(Ordering::Relaxed),
                dropped_frames: e.counters.dropped.load(Ordering::Relaxed),
                ffmpeg_source: e.ffmpeg_source.clone(),
            })
    }

    async fn finalize_via_entry(
        &self,
        entry: Arc<ActiveRecording>,
    ) -> Result<RecordingReceipt, BrowserError> {
        // Best-effort: a dead tab makes this call fail, and that death is
        // often why stop was called. The four-check receipt below is the
        // honesty layer, not this frame.
        let _ = screencast::stop_screencast(&entry.conn, Some(&entry.session)).await;
        let _ = entry.stop_tx.send(true);
        let join = lock(&entry.join).take();
        if let Some(join) = join {
            // The supervisor's own work is budget-bounded; the margin here is
            // for scheduling, not a third budget.
            let bound = self
                .finalize_budget
                .saturating_mul(2)
                .saturating_add(Duration::from_secs(5));
            let _ = tokio::time::timeout(bound, join).await;
        }
        lock(&entry.slot.receipt).take().ok_or_else(|| {
            BrowserError::ActionFailed(format!("recording {} ended without a receipt", entry.id))
        })
    }
}

/// Why the frame loop ended. `Stop` is the only NON-truncating exit.
enum Break {
    Stop,
    StreamClosed,
    /// The recorded tab was destroyed while the engine lived on — the
    /// `Target.targetDestroyed` event arm, which is exactly the shape the
    /// socket-level `StreamClosed` arm cannot see (the C2 residual this
    /// variant closes).
    TabDestroyed,
    WriterDied,
}

struct RunParams {
    conn: CdpConnection,
    session: SessionId,
    events: EventStream,
    child: tokio::process::Child,
    counters: Arc<Counters>,
    stop_rx: watch::Receiver<bool>,
    slot: Arc<ReceiptSlot>,
    path: PathBuf,
    window_start: SystemTime,
    budget: Duration,
    registry: Weak<RecordingRegistry>,
    key: (String, String),
    recording_id: String,
    started: Instant,
}

/// The supervisor: consume `Page.screencastFrame` events into the bounded
/// channel (acking drops itself), watch for stop and for the writer's death,
/// then finalize into exactly one receipt. Delivery is two-lane — the slot
/// (for a stop that took the entry) and the registry's `finished` map (for an
/// ending nobody asked for) — and the lanes cannot both fire, because filing
/// into `finished` requires REMOVING the live entry, which only one side can
/// do first.
async fn run_recording(p: RunParams) {
    let RunParams {
        conn,
        session,
        mut events,
        mut child,
        counters,
        mut stop_rx,
        slot,
        path,
        window_start,
        budget,
        registry,
        key,
        recording_id,
        started,
    } = p;

    let child_stdin = child.stdin.take().expect("ffmpeg stdin is piped");
    let (tx, rx) = mpsc::channel::<screencast::ScreencastFrame>(CHANNEL_CAPACITY);
    // The engine-death signal. NOT the event stream: `events_tx` lives inside
    // the connection's `Shared`, which our own clones keep alive, so the
    // broadcast never closes while we hold them — `fail_all` setting this
    // watch is the only "the socket died" a subscriber can observe.
    let mut engine_gone = conn.closed();
    let mut writer = tokio::spawn(write_frames(
        rx,
        child_stdin,
        conn.clone(),
        session.clone(),
        counters.clone(),
    ));

    let mut writer_result: Option<std::io::Result<()>> = None;
    let mut last_lagged = 0u64;
    let break_kind = loop {
        tokio::select! {
            ev = events.next() => {
                let Some(ev) = ev else { break Break::StreamClosed };
                let lagged = events.lagged();
                if lagged > last_lagged {
                    tracing::warn!(
                        lagged,
                        "recording event stream lagged — frames were dropped by the broadcast, \
                         not by the encoder"
                    );
                    last_lagged = lagged;
                }
                // The tab-death arm: browser-level (no session), so it must be
                // matched BEFORE the session filter below. The tab id of a cdp
                // backend IS the CDP targetId, so the event names this
                // recording's tab directly. The pump's own destroyed arm is
                // the other half of the detector; the supervisor watches the
                // same broadcast itself rather than taking a callback, because
                // the event is already here and a second wire would be a
                // second thing to keep alive.
                if ev.method == "Target.targetDestroyed"
                    && ev.params["targetId"].as_str() == Some(key.1.as_str())
                {
                    break Break::TabDestroyed;
                }
                if ev.method != "Page.screencastFrame" || ev.session.as_ref() != Some(&session) {
                    continue;
                }
                match screencast::screencast_frame(&ev.params) {
                    Ok(frame) => {
                        counters.captured.fetch_add(1, Ordering::Relaxed);
                        match tx.try_send(frame) {
                            Ok(()) => {}
                            Err(mpsc::error::TrySendError::Full(frame)) => {
                                counters.dropped.fetch_add(1, Ordering::Relaxed);
                                // Dropped is also "dealt with": an un-acked
                                // frame stops the engine's stream, and a stall
                                // here is a frozen page, not a smaller file.
                                let _ = screencast::screencast_frame_ack(
                                    &conn,
                                    Some(&session),
                                    frame.session_id,
                                )
                                .await;
                            }
                            Err(mpsc::error::TrySendError::Closed(_)) => {
                                break Break::WriterDied
                            }
                        }
                    }
                    Err(e) => {
                        // The frame counter is all an ack needs — take it even
                        // off an event whose data would not decode.
                        if let Some(sid) = ev.params.get("sessionId").and_then(Value::as_u64) {
                            let _ =
                                screencast::screencast_frame_ack(&conn, Some(&session), sid).await;
                        }
                        tracing::debug!("undecodable screencastFrame, acked and skipped: {e}");
                    }
                }
            }
            _ = stop_rx.changed() => break Break::Stop,
            _ = engine_gone.changed() => break Break::StreamClosed,
            res = &mut writer => {
                writer_result = Some(res.unwrap_or_else(|je| Err(std::io::Error::other(je))));
                break Break::WriterDied;
            }
        }
    };
    drop(tx);

    // Drain the writer (its return drops stdin, closing the pipe), then wait
    // the encoder out — one budget per stage, so the whole finalize is
    // bounded even when both stages stall.
    if writer_result.is_none() {
        match tokio::time::timeout(budget, &mut writer).await {
            Ok(res) => {
                writer_result = Some(res.unwrap_or_else(|je| Err(std::io::Error::other(je))))
            }
            Err(_) => {
                writer.abort();
                writer_result = Some(Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "encoder stopped reading; writer aborted",
                )));
            }
        }
    }

    let (encoder_exit, exit_ok, killed) = match tokio::time::timeout(budget, child.wait()).await {
        Ok(Ok(status)) => {
            let ok = status.code() == Some(0);
            (status_string(&status), ok, false)
        }
        Ok(Err(e)) => (format!("wait error: {e}"), false, false),
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            ("killed".to_string(), false, true)
        }
    };

    let receipt = build_receipt(
        recording_id,
        path,
        started,
        window_start,
        &counters,
        encoder_exit,
        exit_ok,
        killed,
        &break_kind,
        &writer_result,
    );
    *lock(&slot.receipt) = Some(receipt.clone());
    if let Some(registry) = registry.upgrade() {
        // Filing into `finished` requires taking the live entry; if a stop()
        // took it first, that stop is collecting the receipt from the slot
        // and nothing is filed.
        if lock(&registry.live).remove(&key).is_some() {
            lock(&registry.finished).insert(key, receipt);
        }
    }
}

/// The encoder-feeding half. THE pacing rule: a frame's ack leaves only AFTER
/// its bytes reached the encoder's stdin — ack-before-write would let a slow
/// encoder turn into unbounded in-memory buffering (Review Focus #1). Dropped
/// frames never reach this task; the event loop acks those itself.
async fn write_frames(
    mut rx: mpsc::Receiver<screencast::ScreencastFrame>,
    mut stdin: tokio::process::ChildStdin,
    conn: CdpConnection,
    session: SessionId,
    counters: Arc<Counters>,
) -> std::io::Result<()> {
    while let Some(frame) = rx.recv().await {
        stdin.write_all(&frame.data).await?;
        counters.encoded.fetch_add(1, Ordering::Relaxed);
        if let Err(e) =
            screencast::screencast_frame_ack(&conn, Some(&session), frame.session_id).await
        {
            // A failed ack does not stop the encode — the engine's own
            // guillotine ends the stream; the file stays honest.
            tracing::debug!("screencastFrameAck failed ({e}); continuing the encode");
        }
    }
    Ok(())
    // stdin dropped here → the pipe closes → the encoder sees EOF.
}

/// The verified-stop verdict (module doc). `complete` requires ALL of: exit
/// code 0, size > 0, an mtime inside the recording window, and no truncation
/// on the way here. Anything less is `complete = false` with the reason
/// named — a partial file is honest evidence, a fake "complete" is a lie.
#[allow(clippy::too_many_arguments)]
fn build_receipt(
    recording_id: String,
    path: PathBuf,
    started: Instant,
    window_start: SystemTime,
    counters: &Counters,
    encoder_exit: String,
    exit_ok: bool,
    killed: bool,
    break_kind: &Break,
    writer_result: &Option<std::io::Result<()>>,
) -> RecordingReceipt {
    let meta = std::fs::metadata(&path).ok();
    let size_bytes = meta.as_ref().map(|m| m.len()).unwrap_or(0);
    // Filesystem timestamps can tick backwards across a second boundary, so
    // the window's left edge carries a small skew allowance.
    let mtime_fresh = meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .is_some_and(|mt| {
            let lo = window_start
                .checked_sub(Duration::from_secs(2))
                .unwrap_or(window_start);
            let hi = SystemTime::now() + Duration::from_secs(5);
            mt >= lo && mt <= hi
        });
    let mut truncation_reason = match break_kind {
        Break::Stop => None,
        Break::StreamClosed => {
            Some("tab or engine gone: the event stream closed mid-recording".to_string())
        }
        Break::TabDestroyed => Some(
            "the recorded tab was destroyed mid-recording (Target.targetDestroyed)".to_string(),
        ),
        Break::WriterDied => Some(match writer_result {
            Some(Err(e)) => format!("encoder stdin write failed: {e}"),
            _ => "encoder stdin closed unexpectedly".to_string(),
        }),
    };
    if truncation_reason.is_none() && killed {
        truncation_reason =
            Some("encoder did not exit on stdin close; killed after the stop budget".to_string());
    }
    if truncation_reason.is_none() && !exit_ok {
        truncation_reason = Some(format!("encoder exited {encoder_exit}"));
    }
    if truncation_reason.is_none() && size_bytes == 0 {
        truncation_reason = Some("encoder produced no output".to_string());
    }
    if truncation_reason.is_none() && !mtime_fresh {
        truncation_reason = Some("output file's mtime is outside the recording window".to_string());
    }
    let complete = exit_ok && size_bytes > 0 && mtime_fresh && truncation_reason.is_none();
    RecordingReceipt {
        recording_id,
        path,
        duration_ms: started.elapsed().as_millis() as u64,
        captured_frames: counters.captured.load(Ordering::Relaxed),
        encoded_frames: counters.encoded.load(Ordering::Relaxed),
        dropped_frames: counters.dropped.load(Ordering::Relaxed),
        encoder_exit,
        size_bytes,
        complete,
        truncation_reason,
    }
}

fn spawn_ffmpeg(
    ffmpeg: &Path,
    path: &Path,
    fps_cap: u32,
    wire: ScreencastFormat,
    codec_args: &[String],
) -> std::io::Result<tokio::process::Child> {
    // The input codec hint is required: image2pipe cannot probe the codec
    // from an unseekable pipe (measured against ffmpeg n9.0.1: without
    // `-c:v mjpeg` the demuxer reports "unknown codec" and zero streams).
    let input_codec = match wire {
        ScreencastFormat::Jpeg { .. } => "mjpeg",
        ScreencastFormat::Png => "png",
    };
    let mut cmd = tokio::process::Command::new(ffmpeg);
    cmd.arg("-y")
        .arg("-loglevel")
        .arg("error")
        .arg("-f")
        .arg("image2pipe")
        .arg("-c:v")
        .arg(input_codec)
        .arg("-framerate")
        .arg(fps_cap.to_string())
        .arg("-i")
        .arg("-")
        .args(codec_args)
        .arg(path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        // A supervisor that dies without finalizing must not leave an encoder
        // running against a dead pipe.
        .kill_on_drop(true);
    cmd.spawn()
}

fn status_string(status: &std::process::ExitStatus) -> String {
    if let Some(code) = status.code() {
        return code.to_string();
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        format!("signal {}", status.signal().unwrap_or(-1))
    }
    #[cfg(not(unix))]
    {
        "terminated".to_string()
    }
}

/// The `browser_record` verbs, on the one backend that owns a screencast
/// pipe (the trait's other implementors take the default refusal, which
/// names this driver — the `pdf` precedent in `backend.rs`).
impl super::CdpBackend {
    /// `browser_record{action:"start"}` — capability-gated BEFORE anything
    /// is reserved: an engine whose screencast row is not `Supported`
    /// refuses with `UnsupportedByEngine` naming the engine that can, and no
    /// ffmpeg/path/disk check runs against a stream that will never start
    /// (the `route_add` gate's ordering, routes.rs).
    pub(crate) async fn record_start(
        &self,
        tab_id: &str,
        options: RecordStartOptions,
    ) -> Result<RecordingStatus, BrowserError> {
        super::require(
            crate::browser::engine::capabilities(self.engine()),
            self.engine(),
            |c| c.screencast,
            "record_start",
        )?;
        let handle = self.handle().await?;
        let session = handle.ensure_tab(tab_id).await?;
        self.recording_registry()
            .start(self.profile_name(), tab_id, &session, &handle.conn, options)
            .await
            .map_err(|e| match e {
                // A mid-sequence engine refusal keeps its three-class
                // mapping (timeout / protocol / disconnect); the fail-fast
                // variants already carry model-ready text.
                RecordStartError::Engine(cdp) => {
                    super::map_cdp_err(self.engine(), "Page.startScreencast", cdp)
                }
                other => BrowserError::ActionFailed(other.to_string()),
            })
    }

    /// `browser_record{action:"stop"}` — the two entrances of spec §4:
    /// `Some(recording_id)` stops by id, `None` stops the active tab's
    /// recording. Deliberately NOT capability-gated: stop is the off-switch,
    /// and a gate that can keep an off-switch unreachable is fail-dead
    /// (判据 §14; the route list/remove/clear arms make the same call).
    pub(crate) async fn record_stop(
        &self,
        tab_id: &str,
        recording_id: Option<&str>,
    ) -> Result<RecordingReceipt, BrowserError> {
        let registry = self.recording_registry();
        match recording_id {
            Some(id) => registry.stop_by_id(id).await,
            None => registry.stop(self.profile_name(), tab_id).await,
        }
    }

    /// `browser_record{action:"status"}` — the live view, `None` when the
    /// tab is not recording (a plain answer, not an error). Not gated, same
    /// reasoning as [`Self::record_stop`].
    pub(crate) async fn record_status(
        &self,
        tab_id: &str,
    ) -> Result<Option<RecordingStatus>, BrowserError> {
        Ok(self
            .recording_registry()
            .status(self.profile_name(), tab_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- ffmpeg resolution ----------

    #[test]
    fn ffmpeg_resolution_prefers_env_then_path_then_playwright_cache() {
        let env = PathBuf::from("/env/ffmpeg");
        let path = PathBuf::from("/usr/bin/ffmpeg");
        let pw = PathBuf::from("/pw/ffmpeg-linux");

        // All three present → env wins, and the answer NAMES its source
        // (spec §4's `ffmpeg_source` — the label is part of the decision,
        // not a downstream reconstruction).
        assert_eq!(
            pick_ffmpeg(&[
                ("env".into(), Some(env.clone())),
                ("path".into(), Some(path.clone())),
                ("playwright".into(), Some(pw.clone())),
            ]),
            Some(("env".to_string(), env)),
            "ALEPH_FFMPEG outranks everything"
        );
        // env absent → PATH hit wins over the playwright bundle.
        assert_eq!(
            pick_ffmpeg(&[
                ("env".into(), None),
                ("path".into(), Some(path.clone())),
                ("playwright".into(), Some(pw.clone())),
            ]),
            Some(("path".to_string(), path)),
            "PATH outranks the playwright fallback"
        );
        // Only the playwright bundle → it is the answer.
        assert_eq!(
            pick_ffmpeg(&[
                ("env".into(), None),
                ("path".into(), None),
                ("playwright".into(), Some(pw.clone())),
            ]),
            Some(("playwright".to_string(), pw)),
            "the playwright bundle is the last resort, not never"
        );
        // Nothing anywhere → None, and the caller reports the search log.
        assert_eq!(
            pick_ffmpeg(&[("env".into(), None), ("path".into(), None)]),
            None,
        );
    }

    #[test]
    fn resolve_ffmpeg_finds_the_one_on_this_machines_path() {
        // A machine fact the plan relies on: /usr/bin/ffmpeg n9.0.1 is on PATH
        // here, so the resolution chain must come back non-empty with an
        // existing file — and a source label from the chain itself (spec §4's
        // `ffmpeg_source`, not a downstream guess).
        let (found, source) =
            resolve_ffmpeg_labeled().expect("ffmpeg resolves on this machine (PATH or bundle)");
        assert!(
            found.is_file(),
            "resolved ffmpeg must exist: {}",
            found.display()
        );
        assert!(
            ffmpeg_candidates()
                .iter()
                .any(|(label, _)| *label == source),
            "the source label names one of the chain's rungs: {source}"
        );
    }

    // ---------- managed default path ----------

    #[test]
    fn the_default_path_lands_under_the_managed_dir_with_profile_and_tab() {
        let p = default_recording_path("work", "TAB-42", "webm");
        let parent = p.parent().expect("a parent dir");
        assert_eq!(
            parent.file_name().and_then(|s| s.to_str()),
            Some("work"),
            "the profile is the leaf dir: {}",
            parent.display()
        );
        assert_eq!(
            parent
                .parent()
                .and_then(|g| g.file_name())
                .and_then(|s| s.to_str()),
            Some("recordings"),
            "under <aleph home>/recordings/<profile>/: {}",
            parent.display()
        );
        let name = p.file_name().and_then(|s| s.to_str()).expect("file name");
        assert!(
            name.ends_with("-TAB-42.webm"),
            "<utc>-<tab_id>.<ext> shape: {name}"
        );
        // Filesystem-hostile characters in engine-chosen ids are sanitised.
        let weird = default_recording_path("pro/f", "ta/b:1", "mp4");
        let wname = weird
            .file_name()
            .and_then(|s| s.to_str())
            .expect("file name");
        assert!(wname.ends_with("-ta_b_1.mp4"), "sanitised: {wname}");
        assert!(
            !wname.contains('/'),
            "a slash in a tab id must never become a directory: {wname}"
        );
    }

    // ---------- codec args / extension discipline ----------

    #[test]
    fn codec_args_follow_the_extension_and_a_missing_one_is_refused() {
        assert_eq!(
            codec_args_for(Path::new("/tmp/a.webm")).expect("webm"),
            vec!["-c:v".to_string(), "libvpx".to_string()],
            "webm → VP8 (the encoder's name in ffmpeg is `libvpx`, not libvpx-vp8)"
        );
        assert_eq!(
            codec_args_for(Path::new("/tmp/a.mp4")).expect("mp4"),
            vec!["-c:v".to_string(), "libx264".to_string()],
        );
        assert_eq!(
            codec_args_for(Path::new("/tmp/a.mkv")).expect("mkv"),
            Vec::<String>::new(),
            "an unknown-but-present extension is ffmpeg's problem, not a refusal"
        );
        match codec_args_for(Path::new("/tmp/a")) {
            Err(RecordStartError::NoExtension { path }) => {
                assert_eq!(path, PathBuf::from("/tmp/a"));
            }
            other => panic!("a path with no extension must be refused, got {other:?}"),
        }
    }

    #[test]
    fn uniquify_only_moves_aside_for_a_real_collision() {
        let dir = tempfile::tempdir().expect("tmp");
        let fresh = dir.path().join("a.webm");
        assert_eq!(uniquify(fresh.clone()), fresh, "no collision → unchanged");
        std::fs::write(&fresh, b"x").expect("seed file");
        let moved = uniquify(fresh.clone());
        assert_ne!(moved, fresh, "an existing file must not be clobbered");
        assert!(
            moved.to_string_lossy().contains("a-2.webm"),
            "suffix goes before the extension: {}",
            moved.display()
        );
    }

    // ---------- pipeline (FakeCdpServer + real ffmpeg / shell stubs) ----------

    use std::sync::Arc;
    use std::time::Duration;

    use aleph_cdp::testkit::{FakeCdpServer, Responder};
    use aleph_cdp::{CdpConnection, ConnectOptions, SessionId};
    use base64::Engine as _;
    use serde_json::json;

    /// A 64×64 white JPEG produced by `ffmpeg -f lavfi -i color=white:s=64x64
    /// -frames:v 1` (243 bytes) — the smallest real frame this pipeline was
    /// developed against. Embedded so the test does not depend on an
    /// image-generation dependency.
    const JPEG_B64: &str = "/9j/4AAQSkZJRgABAgAAAQABAAD//gAPTGF2YzYzLjEuMTAxAP/bAEMACAoKCwoLDQ0NDQ0NEA8QEBAQEBAQEBAQEBISEhUVFRISEhAQEhIUFBUVFxcXFRUVFRcXGRkZHh4cHCMjJCsrM//EAEsAAQEAAAAAAAAAAAAAAAAAAAAHAQEAAAAAAAAAAAAAAAAAAAAAEAEAAAAAAAAAAAAAAAAAAAAAEQEAAAAAAAAAAAAAAAAAAAAA/8AAEQgAQABAAwEiAAIRAAMRAP/aAAwDAQACEQMRAD8Av4AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAP/Z";

    fn session_id() -> SessionId {
        SessionId("S1".to_string())
    }

    fn opts(path: PathBuf) -> RecordStartOptions {
        RecordStartOptions {
            path: Some(path),
            ..Default::default()
        }
    }

    fn opts_with_ffmpeg(path: PathBuf, ffmpeg: PathBuf) -> RecordStartOptions {
        RecordStartOptions {
            path: Some(path),
            ffmpeg: Some(ffmpeg),
            ..Default::default()
        }
    }

    async fn recording_server() -> (FakeCdpServer, CdpConnection) {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![
            ("Page.startScreencast", Responder::Reply(json!({}))),
            ("Page.stopScreencast", Responder::Reply(json!({}))),
            ("Page.screencastFrameAck", Responder::Reply(json!({}))),
        ]))
        .await;
        let conn = CdpConnection::connect(
            &server.ws_url(),
            ConnectOptions {
                command_timeout: Duration::from_secs(5),
            },
        )
        .await
        .expect("the fake server accepts a websocket");
        (server, conn)
    }

    fn frame_event(n: u64, data_b64: &str) -> serde_json::Value {
        json!({
            "method": "Page.screencastFrame",
            "sessionId": "S1",
            "params": {
                "data": data_b64,
                "sessionId": n,
                "metadata": { "timestamp": 1000.0 + n as f64 }
            }
        })
    }

    fn shell_stub(dir: &tempfile::TempDir, name: &str, body: &str) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, body).expect("write stub");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("chmod stub");
        }
        path
    }

    /// Poll `cond` until it holds or the budget runs out. Timed waits alone
    /// are how flaky tests are made; the condition is the assertion's real
    /// synchronisation point.
    async fn wait_until(mut cond: impl FnMut() -> bool, budget: Duration, what: &str) {
        let deadline = std::time::Instant::now() + budget;
        while std::time::Instant::now() < deadline {
            if cond() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("timed out waiting for: {what}");
    }

    fn ack_ids(server: &FakeCdpServer) -> Vec<u64> {
        server
            .received_for("Page.screencastFrameAck")
            .iter()
            .filter_map(|f| f["params"]["sessionId"].as_u64())
            .collect()
    }

    /// How many video streams ffprobe sees in the file. Runs the REAL ffprobe
    /// (ships with ffmpeg, on PATH here): a file it cannot parse is not a
    /// recording, whatever the exit code said.
    async fn ffprobe_video_streams(path: &Path) -> usize {
        let out = tokio::process::Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "v",
                "-show_entries",
                "stream=codec_type",
                "-of",
                "csv=p=0",
            ])
            .arg(path)
            .output()
            .await
            .expect("ffprobe runs");
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| l.trim() == "video")
            .count()
    }

    fn registry(budget: Duration) -> Arc<RecordingRegistry> {
        Arc::new(RecordingRegistry::new(budget))
    }

    #[tokio::test]
    async fn a_second_start_on_the_same_tab_is_refused_with_the_existing_id() {
        let (_server, conn) = recording_server().await;
        let dir = tempfile::tempdir().expect("tmp");
        let registry = registry(Duration::from_secs(10));

        let first = registry
            .start(
                "p",
                "T1",
                &session_id(),
                &conn,
                opts(dir.path().join("a.webm")),
            )
            .await
            .expect("first start");
        let err = registry
            .start(
                "p",
                "T1",
                &session_id(),
                &conn,
                opts(dir.path().join("b.webm")),
            )
            .await
            .expect_err("a second start on the same tab must be refused");
        match err {
            RecordStartError::AlreadyRecording { existing_id } => {
                assert_eq!(existing_id, first.recording_id);
            }
            other => panic!("expected AlreadyRecording, got {other:?}"),
        }
        // A different tab of the same profile is a different recording.
        registry
            .start(
                "p",
                "T2",
                &session_id(),
                &conn,
                opts(dir.path().join("c.webm")),
            )
            .await
            .expect("a different tab starts fine");
        registry.stop("p", "T1").await.expect("stop T1");
        registry.stop("p", "T2").await.expect("stop T2");
    }

    #[tokio::test]
    async fn start_without_extension_is_refused_before_any_engine_call() {
        let (server, conn) = recording_server().await;
        let dir = tempfile::tempdir().expect("tmp");
        let registry = registry(Duration::from_secs(10));

        let err = registry
            .start(
                "p",
                "T1",
                &session_id(),
                &conn,
                opts(dir.path().join("noext")),
            )
            .await
            .expect_err("no extension must be refused");
        assert!(
            matches!(err, RecordStartError::NoExtension { .. }),
            "expected NoExtension, got {err:?}"
        );
        assert!(registry.status("p", "T1").is_none(), "no residue");
        assert!(
            server.received_for("Page.startScreencast").is_empty(),
            "a refused start must not reach the engine"
        );
    }

    /// Review Focus #4, registry half: an unwritable path is a fail-fast,
    /// measured by an actual probe write — with no half-file and no registry
    /// residue, and a later good start is not poisoned.
    #[tokio::test]
    async fn start_on_an_unwritable_path_fails_fast_without_residue() {
        let (server, conn) = recording_server().await;
        let dir = tempfile::tempdir().expect("tmp");
        let registry = registry(Duration::from_secs(10));

        let err = registry
            .start(
                "p",
                "T1",
                &session_id(),
                &conn,
                opts(dir.path().join("missing-dir").join("out.webm")),
            )
            .await
            .expect_err("a nonexistent parent is not writable");
        assert!(
            matches!(err, RecordStartError::PathNotWritable { .. }),
            "expected PathNotWritable, got {err:?}"
        );
        assert!(registry.status("p", "T1").is_none(), "no residue");
        assert!(
            server.received_for("Page.startScreencast").is_empty(),
            "a refused start must not reach the engine"
        );
        registry
            .start(
                "p",
                "T1",
                &session_id(),
                &conn,
                opts(dir.path().join("good.webm")),
            )
            .await
            .expect("the refusal left nothing behind");
        registry.stop("p", "T1").await.expect("stop");
    }

    #[tokio::test]
    async fn start_then_frames_then_stop_produces_a_playable_file_and_an_honest_receipt() {
        let (server, conn) = recording_server().await;
        let dir = tempfile::tempdir().expect("tmp");
        let out = dir.path().join("session.webm");
        let registry = registry(Duration::from_secs(15));

        let status = registry
            .start("p", "T1", &session_id(), &conn, opts(out.clone()))
            .await
            .expect("start");
        // spec §4's `ffmpeg_source`: start's answer names WHERE the encoder
        // came from, and the label is the resolution chain's own, not a
        // downstream reconstruction. `opts` injects no explicit encoder, so
        // the chain ran — compare against the chain's answer on this machine
        // rather than a literal (PATH vs bundle is a machine fact).
        let (_, chain_label) = resolve_ffmpeg_labeled().expect("ffmpeg resolves on this machine");
        assert_eq!(status.ffmpeg_source, chain_label);
        // …and the live view reports the SAME label, not a re-resolution.
        assert_eq!(
            registry
                .status("p", "T1")
                .expect("live while recording")
                .ffmpeg_source,
            chain_label
        );
        for n in 1..=30 {
            server.push_event(frame_event(n, JPEG_B64));
        }
        wait_until(
            || {
                registry
                    .status("p", "T1")
                    .is_some_and(|s| s.captured_frames == 30)
            },
            Duration::from_secs(5),
            "all 30 frames captured",
        )
        .await;

        let receipt = registry.stop("p", "T1").await.expect("stop");
        assert!(receipt.complete, "receipt must be complete: {receipt:?}");
        // Review Focus #5's reconciliation pin: captured == encoded. Anything
        // less means frames were lost WITHOUT being counted — a silent lie.
        assert_eq!(receipt.captured_frames, 30);
        assert_eq!(
            receipt.encoded_frames, 30,
            "captured == encoded: {receipt:?}"
        );
        assert_eq!(receipt.dropped_frames, 0);
        assert_eq!(receipt.encoder_exit, "0");
        assert!(receipt.size_bytes > 0);
        assert!(receipt.truncation_reason.is_none());
        assert!(receipt.path.is_absolute());
        assert_eq!(receipt.path, out);
        assert!(
            ffprobe_video_streams(&receipt.path).await >= 1,
            "ffprobe must see a video stream in {}",
            receipt.path.display()
        );
    }

    /// Review Focus #1, direction pin: an ack is sent only AFTER the frame's
    /// bytes reached the encoder's stdin. The stub never reads, so the writer
    /// blocks inside `write_all` once the OS pipe is full — frame 1 is 2MB,
    /// larger than any default pipe capacity (/proc/sys/fs/pipe-max-size is
    /// 1MB here). If the ack for frame 1 ever arrives, the ack went out before
    /// the write.
    #[tokio::test]
    async fn ack_is_sent_only_after_the_frame_is_written() {
        let (server, conn) = recording_server().await;
        let dir = tempfile::tempdir().expect("tmp");
        let stub = shell_stub(&dir, "wedged-encoder", "#!/bin/sh\nexec sleep 3600\n");
        let registry = registry(Duration::from_secs(1));

        registry
            .start(
                "p",
                "T1",
                &session_id(),
                &conn,
                opts_with_ffmpeg(dir.path().join("out.webm"), stub),
            )
            .await
            .expect("start");
        let big = base64::engine::general_purpose::STANDARD.encode(vec![0x41u8; 2_000_000]);
        server.push_event(frame_event(1, &big));
        // Let the writer pick frame 1 up and block on the full pipe.
        tokio::time::sleep(Duration::from_millis(500)).await;
        for n in 2..=41 {
            server.push_event(frame_event(n, JPEG_B64));
        }
        wait_until(
            || !server.received_for("Page.screencastFrameAck").is_empty(),
            Duration::from_secs(5),
            "acks for the dropped frames",
        )
        .await;
        let acked = ack_ids(&server);
        assert!(
            !acked.contains(&1),
            "frame 1 is blocked in the encoder's pipe; an ack for it is ack-before-write: {acked:?}"
        );
        assert!(
            acked.iter().all(|id| *id > 1),
            "only frames the loop could not buffer may be acked here: {acked:?}"
        );

        let receipt = registry.stop("p", "T1").await.expect("stop");
        assert_eq!(
            receipt.encoder_exit, "killed",
            "sleep never exits on its own"
        );
        assert!(!receipt.complete);
    }

    /// Review Focus #1, capacity pin: a slow encoder causes DROPS, not growth.
    /// The channel depth is a compile-time constant; the behavioural half is
    /// that every frame is accounted for exactly once — written, or dropped —
    /// and every one of those is acked, so the page never stalls waiting.
    #[tokio::test]
    async fn a_slow_encoder_drops_frames_instead_of_buffering_them() {
        let (server, conn) = recording_server().await;
        let dir = tempfile::tempdir().expect("tmp");
        let stub = shell_stub(
            &dir,
            "slow-encoder",
            "#!/bin/sh\nwhile true; do dd bs=1024 count=1 of=/dev/null 2>/dev/null || break; sleep 0.05; done\n",
        );
        let registry = registry(Duration::from_secs(1));

        registry
            .start(
                "p",
                "T1",
                &session_id(),
                &conn,
                opts_with_ffmpeg(dir.path().join("out.webm"), stub),
            )
            .await
            .expect("start");
        let payload = base64::engine::general_purpose::STANDARD.encode(vec![0x42u8; 2048]);
        // Batched pushes stay under the event broadcast's own capacity (64).
        for chunk in 0..4 {
            for n in (chunk * 50 + 1)..=(chunk * 50 + 50) {
                server.push_event(frame_event(n, &payload));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // The budget is a guard, not the assertion — the condition is the
        // synchronisation point. 10s proved load-sensitive: under a
        // concurrent workspace compile + swap pressure (2026-09-30, this
        // machine) 200 in-process frames timed it out twice while passing
        // in 2.3s alone. 30s keeps the red meaningful (a wedged pipeline
        // still fails fast enough to matter) without flaking on a busy box.
        wait_until(
            || {
                registry
                    .status("p", "T1")
                    .is_some_and(|s| s.captured_frames == 200)
            },
            Duration::from_secs(30),
            "all 200 frames captured",
        )
        .await;
        wait_until(
            || {
                registry
                    .status("p", "T1")
                    .is_some_and(|s| s.dropped_frames > 0)
            },
            Duration::from_secs(5),
            "backpressure drops",
        )
        .await;

        let receipt = registry.stop("p", "T1").await.expect("stop");
        assert_eq!(receipt.captured_frames, 200, "{receipt:?}");
        assert!(
            receipt.dropped_frames > 0,
            "a slow encoder must drop: {receipt:?}"
        );
        assert!(receipt.encoded_frames > 0, "the slow reader does read some");
        // The invariant: every captured frame was acked exactly once — written
        // frames by the writer, dropped frames by the loop. Whatever was still
        // buffered when the wedged writer was aborted is neither, and is
        // counted nowhere.
        let acked = ack_ids(&server).len() as u64;
        assert_eq!(
            acked,
            receipt.encoded_frames + receipt.dropped_frames,
            "acks == encoded + dropped (buffered-at-kill frames are acked to nobody): \
             {acked} acks, {receipt:?}"
        );
    }

    /// Review Focus #2: the tab dies mid-recording. The event stream closes
    /// with the session, the supervisor finalizes on its own, and the next
    /// stop collects a TRUNCATED receipt — never a complete one.
    #[tokio::test]
    async fn tab_death_mid_recording_auto_finalizes_a_truncated_receipt() {
        let (server, conn) = recording_server().await;
        let dir = tempfile::tempdir().expect("tmp");
        let out = dir.path().join("truncated.webm");
        let registry = registry(Duration::from_secs(15));

        registry
            .start("p", "T1", &session_id(), &conn, opts(out.clone()))
            .await
            .expect("start");
        for n in 1..=10 {
            server.push_event(frame_event(n, JPEG_B64));
        }
        wait_until(
            || ack_ids(&server).len() == 10,
            Duration::from_secs(5),
            "all 10 frames written and acked",
        )
        .await;

        // The tab dies: the SOCKET is dropped, which is the connection-death
        // arm — `fail_all` sets the close watch (`engine_gone` above), and
        // THAT is what the supervisor observes. The event stream itself never
        // closes (its sender lives in the connection's `Shared`, kept alive by
        // this loop's own `conn` clones — see the `engine_gone` comment).
        // The OTHER death shape — a tab dying while the ENGINE lives — is the
        // `Target.targetDestroyed` arm's job since C3 Task 1, pinned by
        // `a_target_destroyed_event_finalizes_the_recording_while_the_engine_lives`
        // below; engines that never emit the event still hang that shape until
        // someone stops it (stop's four-check receipt stays honest either way).
        server.drop_socket();
        // The supervisor notices the closed stream and retires the entry
        // itself — wait for THAT, so the receipt below is the auto-finalized
        // one, not a stop-triggered one.
        wait_until(
            || registry.status("p", "T1").is_none(),
            Duration::from_secs(10),
            "the supervisor to auto-finalize the dead tab's recording",
        )
        .await;

        let receipt = registry
            .stop_by_id("rec1")
            .await
            .expect("the truncated receipt waits for its stop");
        assert!(
            !receipt.complete,
            "a dead tab's file is never complete: {receipt:?}"
        );
        assert!(
            receipt.truncation_reason.is_some(),
            "the truncation is named: {receipt:?}"
        );
        assert_eq!(receipt.encoded_frames, 10);
        assert_eq!(
            receipt.encoder_exit, "0",
            "the encoder finished its half cleanly"
        );
        assert!(receipt.size_bytes > 0);
        assert!(
            ffprobe_video_streams(&receipt.path).await >= 1,
            "the honest first half is a playable file"
        );
        // A receipt is collected once.
        let again = registry.stop("p", "T1").await;
        assert!(
            matches!(again, Err(BrowserError::NoActiveRecording(_))),
            "a second stop finds nothing: {again:?}"
        );
    }

    /// Review Focus #3: exit 1 with a non-empty file on disk is NOT complete.
    #[tokio::test]
    async fn a_nonzero_encoder_exit_is_never_reported_as_complete() {
        let (server, conn) = recording_server().await;
        let dir = tempfile::tempdir().expect("tmp");
        let stub = shell_stub(
            &dir,
            "failing-encoder",
            "#!/bin/sh\nfor last; do :; done\ncat > \"$last\"\nexit 1\n",
        );
        let registry = registry(Duration::from_secs(10));

        let status = registry
            .start(
                "p",
                "T1",
                &session_id(),
                &conn,
                opts_with_ffmpeg(dir.path().join("out.webm"), stub),
            )
            .await
            .expect("start");
        // The explicit-injection seam is labeled as such — a stub-encoder
        // test reading "ffmpeg on PATH" here would be the label lying about
        // the decision that was actually made.
        assert!(
            status.ffmpeg_source.contains("explicit"),
            "the injected encoder's source label must say so: {}",
            status.ffmpeg_source
        );
        for n in 1..=5 {
            server.push_event(frame_event(n, JPEG_B64));
        }
        wait_until(
            || {
                registry
                    .status("p", "T1")
                    .is_some_and(|s| s.captured_frames == 5)
            },
            Duration::from_secs(5),
            "frames captured",
        )
        .await;

        let receipt = registry.stop("p", "T1").await.expect("stop");
        assert_eq!(receipt.encoder_exit, "1");
        assert!(
            !receipt.complete,
            "exit 1 is never complete, even with a non-empty file: {receipt:?}"
        );
        assert!(
            receipt.size_bytes > 0,
            "the file IS there — and it still does not count"
        );
    }

    /// The stop half of a wedged encoder: stdin closes, the encoder ignores
    /// it, the bounded wait expires, and the receipt says `killed`.
    #[tokio::test]
    async fn stop_kills_a_wedged_ffmpeg_and_says_so() {
        let (server, conn) = recording_server().await;
        let dir = tempfile::tempdir().expect("tmp");
        let stub = shell_stub(&dir, "wedged-encoder", "#!/bin/sh\nexec sleep 3600\n");
        let registry = registry(Duration::from_secs(1));

        registry
            .start(
                "p",
                "T1",
                &session_id(),
                &conn,
                opts_with_ffmpeg(dir.path().join("out.webm"), stub),
            )
            .await
            .expect("start");
        for n in 1..=3 {
            server.push_event(frame_event(n, JPEG_B64));
        }
        let receipt = registry
            .stop("p", "T1")
            .await
            .expect("stop returns, bounded");
        assert_eq!(receipt.encoder_exit, "killed", "{receipt:?}");
        assert!(!receipt.complete);
        assert!(receipt.truncation_reason.is_some());
    }

    /// The encoder dies mid-recording; the NEXT frame's write fails and that
    /// failure — not a timeout, not a stop — ends the recording. Note the
    /// honest combination this pins: the stub exits 0, yet the receipt is
    /// truncated, because the exit code is only one of four checks.
    #[tokio::test]
    async fn ffmpeg_death_mid_recording_is_detected_on_the_next_frame_write() {
        let (server, conn) = recording_server().await;
        let dir = tempfile::tempdir().expect("tmp");
        let stub = shell_stub(
            &dir,
            "short-lived-encoder",
            "#!/bin/sh\nhead -c 256 >/dev/null\nexit 0\n",
        );
        let registry = registry(Duration::from_secs(10));

        registry
            .start(
                "p",
                "T1",
                &session_id(),
                &conn,
                opts_with_ffmpeg(dir.path().join("out.webm"), stub),
            )
            .await
            .expect("start");
        for n in 1..=10 {
            server.push_event(frame_event(n, JPEG_B64));
            tokio::time::sleep(Duration::from_millis(80)).await;
            if registry.status("p", "T1").is_none() {
                break;
            }
        }
        wait_until(
            || registry.status("p", "T1").is_none(),
            Duration::from_secs(10),
            "the write failure to retire the recording",
        )
        .await;

        let receipt = registry.stop("p", "T1").await.expect("the filed receipt");
        assert!(!receipt.complete, "{receipt:?}");
        assert!(
            receipt
                .truncation_reason
                .as_deref()
                .is_some_and(|r| r.contains("stdin")),
            "the write failure is the named cause: {receipt:?}"
        );
        assert_eq!(
            receipt.encoder_exit, "0",
            "the stub exited 0 — and it changes nothing"
        );
    }

    /// The mtime check's own pin: an exit-0 encoder that leaves a PRE-EXISTING
    /// file untouched must not produce "complete" — the file's mtime is
    /// outside the recording window. Without the check this exact shape (old
    /// file at the path) is a silent complete-lie.
    #[tokio::test]
    async fn a_preexisting_stale_file_is_never_reported_complete() {
        let (server, conn) = recording_server().await;
        let dir = tempfile::tempdir().expect("tmp");
        let out = dir.path().join("stale.webm");
        std::fs::write(&out, b"old content from an earlier run").expect("seed file");
        std::fs::File::options()
            .write(true)
            .open(&out)
            .expect("open")
            .set_modified(std::time::SystemTime::now() - Duration::from_secs(3600))
            .expect("backdate mtime");
        let stub = shell_stub(
            &dir,
            "no-touch-encoder",
            "#!/bin/sh\ncat >/dev/null\nexit 0\n",
        );
        let registry = registry(Duration::from_secs(10));

        registry
            .start(
                "p",
                "T1",
                &session_id(),
                &conn,
                opts_with_ffmpeg(out.clone(), stub),
            )
            .await
            .expect("start");
        server.push_event(frame_event(1, JPEG_B64));
        wait_until(
            || {
                registry
                    .status("p", "T1")
                    .is_some_and(|s| s.captured_frames == 1)
            },
            Duration::from_secs(5),
            "frame captured",
        )
        .await;

        let receipt = registry.stop("p", "T1").await.expect("stop");
        assert_eq!(receipt.encoder_exit, "0");
        assert!(receipt.size_bytes > 0, "the old file is still there");
        assert!(
            !receipt.complete,
            "an untouched pre-existing file is not a recording: {receipt:?}"
        );
        assert!(
            receipt
                .truncation_reason
                .as_deref()
                .is_some_and(|r| r.contains("mtime")),
            "the mtime check is what fired: {receipt:?}"
        );
    }

    /// The T3 wiring seam: the per-call backend carries the manager-owned
    /// registry, so a verb on any one call's backend sees the same recordings.
    #[tokio::test]
    async fn the_backend_carries_the_shared_registry() {
        use super::super::test_support::{backend_with, open_guard};
        use crate::browser::engine::Engine;

        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        let (_reg, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;
        assert!(backend
            .recording_registry()
            .status("default", "T1")
            .is_none());
    }

    /// **The C3 event arm: the TAB dies while the ENGINE lives.** The socket
    /// stays up, so the `conn.closed()` watch never fires; the supervisor's
    /// own `Target.targetDestroyed` match is what ends the recording, and the
    /// filed receipt is a truncation, collectable by `stop_by_id`.
    #[tokio::test]
    async fn a_target_destroyed_event_finalizes_the_recording_while_the_engine_lives() {
        let (server, conn) = recording_server().await;
        let dir = tempfile::tempdir().expect("tmp");
        let out = dir.path().join("tabdied.webm");
        let registry = registry(Duration::from_secs(15));

        registry
            .start("p", "T1", &session_id(), &conn, opts(out.clone()))
            .await
            .expect("start");
        for n in 1..=5 {
            server.push_event(frame_event(n, JPEG_B64));
        }
        wait_until(
            || ack_ids(&server).len() == 5,
            Duration::from_secs(5),
            "all 5 frames written and acked",
        )
        .await;

        // The tab dies; the socket — and so the engine — stays up, which is
        // exactly the shape the socket-level arm cannot see.
        server.push_event(json!({
            "method": "Target.targetDestroyed",
            "params": { "targetId": "T1" }
        }));
        wait_until(
            || registry.status("p", "T1").is_none(),
            Duration::from_secs(10),
            "the supervisor to auto-finalize the destroyed tab's recording",
        )
        .await;

        let receipt = registry
            .stop_by_id("rec1")
            .await
            .expect("the truncated receipt waits for its stop");
        assert!(
            !receipt.complete,
            "a dead tab's file is never complete: {receipt:?}"
        );
        assert!(
            receipt
                .truncation_reason
                .as_deref()
                .is_some_and(|r| r.contains("destroyed")),
            "the tab's destruction is the named cause: {receipt:?}"
        );
        assert_eq!(receipt.encoded_frames, 5);
        assert_eq!(
            receipt.encoder_exit, "0",
            "the encoder finished its half cleanly"
        );
        assert!(receipt.size_bytes > 0);
        assert!(
            ffprobe_video_streams(&receipt.path).await >= 1,
            "the honest first half is a playable file"
        );
    }

    /// The arm names ONE tab: a destroyed event for a DIFFERENT tab must not
    /// end this recording. Pinned positively — the recording goes on to a
    /// complete stop — because a sleep-then-assert-alive negative would
    /// measure the scheduler, not the filter.
    #[tokio::test]
    async fn a_destroyed_event_for_another_tab_leaves_the_recording_running() {
        let (server, conn) = recording_server().await;
        let dir = tempfile::tempdir().expect("tmp");
        let out = dir.path().join("stillalive.webm");
        let registry = registry(Duration::from_secs(15));

        registry
            .start("p", "T1", &session_id(), &conn, opts(out.clone()))
            .await
            .expect("start");
        for n in 1..=5 {
            server.push_event(frame_event(n, JPEG_B64));
        }
        server.push_event(json!({
            "method": "Target.targetDestroyed",
            "params": { "targetId": "T-OTHER" }
        }));
        // Proof the loop kept consuming past the foreign death: one more
        // frame, captured and acked.
        server.push_event(frame_event(6, JPEG_B64));
        wait_until(
            || ack_ids(&server).len() == 6,
            Duration::from_secs(5),
            "the frame after the foreign death is still written and acked",
        )
        .await;
        assert!(
            registry.status("p", "T1").is_some(),
            "another tab's death must not retire this recording"
        );

        let receipt = registry.stop("p", "T1").await.expect("stop");
        assert!(
            receipt.complete,
            "an undisturbed recording completes: {receipt:?}"
        );
        assert_eq!(receipt.encoded_frames, 6);
    }
}
