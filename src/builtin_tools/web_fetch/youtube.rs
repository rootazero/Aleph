//! YouTube transcript extraction for `web_fetch`.
//!
//! Dispatched from `call_impl` before the HTTP fetch, gated by
//! `[policies.web_fetch] youtube_transcript` (default on).
//!
//! Pipeline: [`detect_youtube`] decides whether a URL is a YouTube *video*
//! page (the single dispatch-decision entry point), [`fetch_transcript`]
//! pulls the subtitle track via `yt-dlp` and [`clean_vtt`] reduces WEBVTT
//! cues to flowing plain text.
//!
//! The only external side effect (spawning `yt-dlp`) sits behind the
//! [`YtDlpRunner`] trait so tests inject a fake and never need a real
//! binary or network access.

use async_trait::async_trait;
use once_cell::sync::Lazy;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::debug;

/// Wall-clock budget for a single `yt-dlp` subtitle pull.
const DEFAULT_YTDLP_TIMEOUT: Duration = Duration::from_secs(30);

/// Hard cap on the raw subtitle stream pulled from `yt-dlp` stdout.
/// Multi-hour talks produce ~2-3 MiB of VTT; 8 MiB leaves ample headroom.
const MAX_SUBTITLE_BYTES: usize = 8 * 1024 * 1024;

/// Preferred subtitle languages, in priority order (yt-dlp regex syntax):
/// any English variant first, then the common Chinese variants.
const SUB_LANGS: &str = "en.*,zh-Hans,zh-Hant,zh-CN,zh-TW,zh.*";

/// Gap between adjacent cues (seconds) that triggers a paragraph break.
const PARAGRAPH_GAP_SECS: f64 = 1.5;

/// Maximum lines per paragraph before forcing a break, so transcripts of
/// continuous speech still get visual structure.
const MAX_PARAGRAPH_LINES: usize = 24;

// ─── Errors ────────────────────────────────────────────────────────────────

/// Failure modes of the YouTube transcript path.
///
/// "Soft" failures ([`YoutubeError::YtDlpUnavailable`],
/// [`YoutubeError::NoSubtitles`]) mean the YouTube path cannot produce a
/// transcript and the caller should fall back to the generic HTTP fetch
/// path. "Hard" failures mean the YouTube path was reachable and failed —
/// falling back to generic HTML extraction of a watch page rarely yields
/// useful content, so the caller should surface the error instead.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum YoutubeError {
    /// `yt-dlp` is not installed / not resolvable on PATH.
    #[error("yt-dlp is not available (not installed or not on PATH)")]
    YtDlpUnavailable,

    /// `yt-dlp` ran but failed (non-zero exit, timeout, oversized output).
    /// Never carries raw stderr: yt-dlp stderr can embed the request URL
    /// and local cookie paths, so it is logged at debug level only.
    #[error("yt-dlp fetch failed: {0}")]
    FetchFailed(String),

    /// The video has no manual or automatic subtitles in the preferred
    /// languages. Soft: the generic HTTP path may still yield a useful page.
    #[error("no subtitles available in the preferred languages")]
    NoSubtitles,

    /// `yt-dlp` output was not parseable as WEBVTT.
    #[error("could not parse yt-dlp subtitle output: {0}")]
    ParseFailed(String),
}

impl YoutubeError {
    /// Whether the caller may sensibly fall back to the generic HTTP path.
    #[must_use]
    pub(crate) const fn is_soft(&self) -> bool {
        matches!(self, Self::YtDlpUnavailable | Self::NoSubtitles)
    }
}

// ─── URL detection ─────────────────────────────────────────────────────────

/// A validated YouTube video reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct YouTubeTarget {
    video_id: String,
}

impl YouTubeTarget {
    /// The 11-character video id.
    #[cfg(test)]
    pub(crate) fn video_id(&self) -> &str {
        &self.video_id
    }

    /// Canonical watch URL handed to `yt-dlp`.
    #[must_use]
    pub(crate) fn canonical_url(&self) -> String {
        format!("https://www.youtube.com/watch?v={}", self.video_id)
    }
}

/// A valid YouTube video id is exactly 11 chars of the base64url alphabet.
fn is_valid_video_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// True for the YouTube hosts that serve video watch pages. Note this is an
/// exact-match allowlist, not a suffix check, so `evil-youtube.com` and
/// `youtube.com.evil.test` do not pass.
fn is_youtube_host(host: &str) -> bool {
    matches!(
        host,
        "youtube.com" | "www.youtube.com" | "m.youtube.com" | "music.youtube.com"
    )
}

/// Detect whether `url` points at a YouTube *video* and extract its id.
///
/// Recognized shapes: `youtube.com/watch?v=ID`, `youtu.be/ID`,
/// `youtube.com/shorts/ID`, `youtube.com/embed/ID`, across the `www`/`m`/
/// `music` subdomains. Non-video YouTube pages (homepage, `/channel/…`,
/// `/playlist`, `/results`, …) and non-YouTube URLs return `None`.
///
/// This is the single dispatch-decision entry point for the web_fetch
/// integration: `Some(_)` means "route to the transcript path".
#[must_use]
pub(crate) fn detect_youtube(url: &str) -> Option<YouTubeTarget> {
    let parsed = url::Url::parse(url).ok()?;
    // `host_str` is already lower-cased per the URL spec.
    let host = parsed.host_str()?;

    let id = if host == "youtu.be" || host == "www.youtu.be" {
        // youtu.be/ID — the id is the entire single path segment.
        let path = parsed.path().trim_start_matches('/');
        let (segment, rest) = path.split_once('/').unwrap_or((path, ""));
        if !rest.is_empty() {
            return None; // youtu.be/ID/extra is not a canonical video link
        }
        segment
    } else if is_youtube_host(host) {
        let mut segments = parsed.path_segments()?;
        match (segments.next(), segments.next()) {
            // /watch?v=ID
            (Some("watch"), _) => {
                return parsed
                    .query_pairs()
                    .find(|(k, _)| k == "v")
                    .map(|(_, v)| v.into_owned())
                    .filter(|id| is_valid_video_id(id))
                    .map(|video_id| YouTubeTarget { video_id });
            }
            // /shorts/ID, /embed/ID (extra trailing segments tolerated)
            (Some("shorts" | "embed"), Some(id)) => id,
            // "/", "/channel/…", "/playlist", "/results", … — not videos.
            _ => return None,
        }
    } else {
        return None;
    };

    if is_valid_video_id(id) {
        Some(YouTubeTarget {
            video_id: id.to_string(),
        })
    } else {
        None
    }
}

// ─── yt-dlp execution boundary ─────────────────────────────────────────────

/// Completed output of a single `yt-dlp` child process.
///
/// Deliberately carries NO stderr: yt-dlp stderr can embed the request URL
/// and local cookie paths, so the runner logs a sanitized tail at debug
/// level and the field never crosses the boundary. This makes the
/// no-stderr-in-error-message rule structural rather than conventional.
#[derive(Debug, Clone)]
pub(crate) struct ProcessOutput {
    pub(crate) success: bool,
    pub(crate) code: Option<i32>,
    pub(crate) stdout: String,
}

/// Injectable boundary around "run yt-dlp with these args". Tests provide a
/// fake; production uses [`YtDlpCommand`].
#[async_trait]
pub(crate) trait YtDlpRunner: Send + Sync {
    async fn run(&self, args: &[String]) -> std::result::Result<ProcessOutput, YoutubeError>;
}

/// Default [`YtDlpRunner`]: spawns the real `yt-dlp` binary with a
/// wall-clock timeout and `kill_on_drop` so a timed-out child cannot leak.
pub(crate) struct YtDlpCommand {
    bin: PathBuf,
    timeout: Duration,
}

impl YtDlpCommand {
    /// Locate `yt-dlp` on PATH.
    ///
    /// NOTE for the wiring commit: `crate::runtimes` has no `yt-dlp` spec
    /// registered today, so `runtimes::probe::probe("yt-dlp")` always
    /// reports not-found and cannot be used here. Once a `RuntimeSpec` for
    /// yt-dlp exists, prefer the runtimes facility over this PATH scan.
    pub(crate) fn resolve() -> Option<Self> {
        resolve_ytdlp_binary().map(|bin| Self {
            bin,
            timeout: DEFAULT_YTDLP_TIMEOUT,
        })
    }

    /// Construct a runner for an explicit binary path and timeout.
    /// Test-only seam for pointing the runner at stand-in binaries.
    #[cfg(test)]
    pub(crate) fn with_binary(bin: PathBuf, timeout: Duration) -> Self {
        Self { bin, timeout }
    }
}

#[async_trait]
impl YtDlpRunner for YtDlpCommand {
    async fn run(&self, args: &[String]) -> std::result::Result<ProcessOutput, YoutubeError> {
        use crate::utils::no_window::NoWindow;

        let mut cmd = tokio::process::Command::new(&self.bin);
        cmd.args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .no_window();

        match tokio::time::timeout(self.timeout, cmd.output()).await {
            Err(_) => Err(YoutubeError::FetchFailed(format!(
                "yt-dlp timed out after {}s",
                self.timeout.as_secs()
            ))),
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(YoutubeError::YtDlpUnavailable)
            }
            Ok(Err(e)) => Err(YoutubeError::FetchFailed(format!("spawn failed: {e}"))),
            Ok(Ok(out)) => {
                if !out.status.success() {
                    // Debug-log only: stderr may embed the URL and cookie
                    // paths; it must not reach the error surface.
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    debug!(
                        code = ?out.status.code(),
                        stderr_tail = %stderr_tail(&stderr),
                        "yt-dlp exited non-zero"
                    );
                }
                Ok(ProcessOutput {
                    success: out.status.success(),
                    code: out.status.code(),
                    stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                })
            }
        }
    }
}

/// Minimal PATH scan for the `yt-dlp` binary. Deliberately NOT
/// `runtimes::probe` — see [`YtDlpCommand::resolve`].
fn resolve_ytdlp_binary() -> Option<PathBuf> {
    let exe: &Path = if cfg!(windows) {
        Path::new("yt-dlp.exe")
    } else {
        Path::new("yt-dlp")
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(exe))
        .find(|p| p.is_file())
}

/// Last ~200 chars of stderr for debug logging, boundary-safe.
fn stderr_tail(stderr: &str) -> &str {
    let trimmed = stderr.trim();
    let mut start = trimmed.len().saturating_sub(200);
    while start > 0 && !trimmed.is_char_boundary(start) {
        start -= 1;
    }
    &trimmed[start..]
}

/// Build the yt-dlp argument vector for a transcript pull.
///
/// Chosen shape: stream the subtitle file to stdout (`-o -`) in a single
/// process invocation, rather than `-J`/`--dump-single-json` + a second
/// HTTP fetch of the timedtext URL. Rationale:
///
/// - One side-effecting step, no tempdir, no second network client to
///   SSRF-audit (the timedtext URL would arrive from untrusted tool
///   output and need host validation).
/// - `--write-subs --write-auto-subs` together means "manual subtitles if
///   present, automatic captions otherwise" (yt-dlp skips auto captions
///   for languages that already have manual subs).
/// - `--sub-format vtt/best` pins VTT when offered so [`clean_vtt`]
///   applies; `best` is the fallback for exotic tracks.
/// - Multiple matching languages can concatenate several VTT documents on
///   stdout; [`first_vtt_document`] keeps only the first, which follows
///   yt-dlp's language-preference order.
fn ytdlp_args(url: &str) -> Vec<String> {
    [
        "--no-playlist",
        "--no-warnings",
        "--skip-download",
        "--write-subs",
        "--write-auto-subs",
        "--sub-langs",
        SUB_LANGS,
        "--sub-format",
        "vtt/best",
        "-o",
        "-",
        "--",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .chain(std::iter::once(url.to_string()))
    .collect()
}

// ─── Transcript fetch ──────────────────────────────────────────────────────

/// A cleaned, plain-text video transcript.
#[derive(Debug, Clone)]
pub(crate) struct YouTubeTranscript {
    text: String,
}

impl YouTubeTranscript {
    #[must_use]
    pub(crate) fn text(&self) -> &str {
        &self.text
    }
}

/// Production entry: resolve `yt-dlp` from PATH, then fetch.
///
/// Not exercised by unit tests because it needs a real binary and network.
pub(crate) async fn fetch_transcript(
    target: &YouTubeTarget,
) -> std::result::Result<YouTubeTranscript, YoutubeError> {
    let runner = YtDlpCommand::resolve().ok_or(YoutubeError::YtDlpUnavailable)?;
    fetch_transcript_with(&runner, target).await
}

/// Full pipeline behind the injectable runner: yt-dlp → first VTT document
/// → cleaned plain text.
pub(crate) async fn fetch_transcript_with(
    runner: &impl YtDlpRunner,
    target: &YouTubeTarget,
) -> std::result::Result<YouTubeTranscript, YoutubeError> {
    let args = ytdlp_args(&target.canonical_url());
    let out = runner.run(&args).await?;

    if !out.success {
        return Err(YoutubeError::FetchFailed(match out.code {
            Some(code) => format!("yt-dlp exited with code {code}"),
            None => "yt-dlp terminated without an exit code".to_string(),
        }));
    }

    if out.stdout.len() > MAX_SUBTITLE_BYTES {
        return Err(YoutubeError::FetchFailed(format!(
            "subtitle output exceeded {} bytes",
            MAX_SUBTITLE_BYTES
        )));
    }

    let raw = out.stdout.strip_prefix('\u{feff}').unwrap_or(&out.stdout);
    if raw.trim().is_empty() {
        // yt-dlp exits 0 with empty stdout when the video has no matching
        // subtitle track (the "video doesn't have subtitles" notice goes
        // to stderr only).
        return Err(YoutubeError::NoSubtitles);
    }

    let doc = first_vtt_document(raw);
    if !doc.starts_with("WEBVTT") {
        return Err(YoutubeError::ParseFailed(
            "yt-dlp stdout did not start with a WEBVTT header".to_string(),
        ));
    }

    let text = clean_vtt(doc);
    if text.is_empty() {
        return Err(YoutubeError::NoSubtitles);
    }

    Ok(YouTubeTranscript { text })
}

/// When several languages match, yt-dlp concatenates multiple VTT files on
/// stdout. Keep only the first document (language-preference order).
fn first_vtt_document(raw: &str) -> &str {
    match raw.find("\nWEBVTT") {
        Some(i) => &raw[..i],
        None => raw,
    }
}

// ─── VTT cleaning (pure) ───────────────────────────────────────────────────

/// Matches any `<…>` inline tag: `<c>`, `<c.colorE5E5E5>`, `</c>`,
/// inline timestamp tags like `<00:00:01.500>`, and formatting tags
/// (`<b>`, `<i>`, `<u>`, `<ruby>`, `<rt>`). VTT requires literal `<` in
/// payload text to be escaped as `&lt;`, so a blanket strip is safe.
static INLINE_TAG_RE: Lazy<regex::Regex> = Lazy::new(|| {
    // rust-doctor-disable-next-line panic-in-library
    regex::Regex::new(r"<[^>]*>").expect("static inline-tag regex must compile")
});

/// One parsed cue: timing plus its cleaned payload lines.
#[derive(Debug)]
struct Cue {
    start: f64,
    end: f64,
    lines: Vec<String>,
}

/// Clean a WEBVTT subtitle document into flowing plain text.
///
/// Rules, in order:
/// 1. Drop the `WEBVTT` header block (incl. `Kind:`/`Language:` lines),
///    `NOTE`/`STYLE`/`REGION` blocks, and cue identifier lines — any block
///    without a `-->` timestamp line carries no transcript text.
/// 2. Strip all `<…>` inline tags (class, formatting, inline timestamps).
/// 3. Decode the six entities YouTube actually emits (`&amp;`, `&lt;`,
///    `&gt;`, `&quot;`, `&#39;`, `&nbsp;`).
/// 4. Trim payload lines and drop empty cues.
/// 5. Rolling-overlap dedup: YouTube auto-captions re-emit the previous
///    cue's tail as the next cue's head; the shared suffix/prefix is
///    dropped (this alone typically halves auto-sub output).
/// 6. Adjacent exact-duplicate line collapse (manual subs with repeated
///    lines).
/// 7. Paragraph shaping: lines join with spaces; a blank line is emitted
///    when the gap between consecutive cues exceeds
///    [`PARAGRAPH_GAP_SECS`] or a paragraph reaches
///    [`MAX_PARAGRAPH_LINES`].
#[must_use]
pub(crate) fn clean_vtt(vtt: &str) -> String {
    let cues = parse_cues(vtt);

    let mut paragraphs: Vec<String> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut prev_full_lines: Vec<String> = Vec::new();
    let mut prev_end: Option<f64> = None;
    let mut last_emitted: Option<String> = None;

    for cue in &cues {
        // Rule 5: drop the prefix this cue shares with the previous cue's
        // full (pre-dedup) lines — the rolling-caption overlap.
        let skip = overlap_prefix(&prev_full_lines, &cue.lines);
        prev_full_lines = cue.lines.clone();

        let new_lines = &cue.lines[skip..];
        if new_lines.is_empty() {
            prev_end = Some(cue.end);
            continue;
        }

        // Rule 7: paragraph break before emitting this cue's lines.
        let gap_break = prev_end.is_some_and(|end| cue.start - end > PARAGRAPH_GAP_SECS);
        if !current.is_empty() && (gap_break || current.len() >= MAX_PARAGRAPH_LINES) {
            paragraphs.push(current.join(" "));
            current.clear();
        }

        for line in new_lines {
            // Rule 6: adjacent exact-duplicate collapse.
            if last_emitted.as_deref() == Some(line.as_str()) {
                continue;
            }
            current.push(line.clone());
            last_emitted = Some(line.clone());
        }
        prev_end = Some(cue.end);
    }

    if !current.is_empty() {
        paragraphs.push(current.join(" "));
    }
    paragraphs.join("\n\n")
}

/// Length of the longest prefix of `cur` that equals a suffix of `prev`.
fn overlap_prefix(prev: &[String], cur: &[String]) -> usize {
    let max = prev.len().min(cur.len());
    (1..=max)
        .rev()
        .find(|&k| prev[prev.len() - k..] == cur[..k])
        .unwrap_or(0)
}

/// Split a WEBVTT document into cues with cleaned payload lines
/// (rules 1-4 of [`clean_vtt`]).
fn parse_cues(vtt: &str) -> Vec<Cue> {
    let normalized = vtt.replace("\r\n", "\n");
    let mut cues = Vec::new();

    for block in normalized.split("\n\n") {
        let lines: Vec<&str> = block.lines().collect();
        // The timestamp line is the block's anchor; everything before it
        // (WEBVTT header text, NOTE, cue identifiers) is dropped.
        let Some(ts_idx) = lines.iter().position(|l| l.contains(" --> ")) else {
            continue;
        };
        let Some((start, end)) = parse_timestamp_line(lines[ts_idx]) else {
            continue;
        };

        let payload: Vec<String> = lines[ts_idx + 1..]
            .iter()
            .map(|l| decode_entities(&INLINE_TAG_RE.replace_all(l, "")))
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();

        if !payload.is_empty() {
            cues.push(Cue {
                start,
                end,
                lines: payload,
            });
        }
    }
    cues
}

/// Parse `HH:MM:SS.mmm --> HH:MM:SS.mmm <cue settings>` into seconds.
fn parse_timestamp_line(line: &str) -> Option<(f64, f64)> {
    let (start, rest) = line.split_once(" --> ")?;
    // Cue settings follow the end timestamp after whitespace.
    let end = rest.split_whitespace().next()?;
    Some((parse_timestamp(start.trim())?, parse_timestamp(end)?))
}

/// Parse `MM:SS.mmm` or `HH:MM:SS.mmm` into seconds.
fn parse_timestamp(ts: &str) -> Option<f64> {
    let mut parts = ts.split(':');
    let first = parts.next()?;
    let second = parts.next()?;
    let (hours, minutes, secs) = match parts.next() {
        Some(third) => (
            first.parse::<f64>().ok()?,
            second.parse::<f64>().ok()?,
            third,
        ),
        None => (0.0, first.parse::<f64>().ok()?, second),
    };
    if parts.next().is_some() {
        return None; // too many segments
    }
    let secs: f64 = secs.parse().ok()?;
    Some(hours * 3600.0 + minutes * 60.0 + secs)
}

/// Decode the entities YouTube subtitle tracks actually contain.
/// `&amp;` is decoded last so `&amp;lt;` becomes `&lt;` (single pass).
fn decode_entities(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // ─── URL detection matrix ────────────────────────────────────────────

    #[test]
    fn detect_accepts_video_urls() {
        let cases = [
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://youtube.com/watch?v=dQw4w9WgXcQ",
            "http://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=42s&list=PLabc",
            "https://m.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://music.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ?t=42",
            "https://www.youtube.com/shorts/dQw4w9WgXcQ",
            "https://youtube.com/embed/dQw4w9WgXcQ",
            "https://www.youtube.com/embed/dQw4w9WgXcQ?rel=0",
            // base64url alphabet: '-' and '_' are legal
            "https://youtu.be/a-b_c123456",
        ];
        for url in cases {
            let target = detect_youtube(url).unwrap_or_else(|| panic!("expected hit: {url}"));
            assert_eq!(target.video_id().len(), 11, "id length for {url}");
        }
        let t = detect_youtube("https://youtu.be/dQw4w9WgXcQ").unwrap();
        assert_eq!(t.video_id(), "dQw4w9WgXcQ");
        assert_eq!(
            t.canonical_url(),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        );
    }

    #[test]
    fn detect_rejects_non_video_and_non_youtube() {
        let cases = [
            // YouTube non-video pages
            "https://www.youtube.com/",
            "https://www.youtube.com/channel/UCuAXFkgsw1L7xaCfnd5JJOw",
            "https://www.youtube.com/@LinusTechTips",
            "https://www.youtube.com/playlist?list=PLrAXtmErZgOeiKm4sgNOknGvNjby9efdf",
            "https://www.youtube.com/results?search_query=rust",
            "https://www.youtube.com/feed/subscriptions",
            "https://music.youtube.com/",
            // watch without a v param
            "https://www.youtube.com/watch",
            "https://www.youtube.com/watch?list=PLabc",
            // malformed ids
            "https://youtu.be/dQw4w9WgXc",        // 10 chars
            "https://youtu.be/dQw4w9WgXcQQ",      // 12 chars
            "https://youtu.be/dQw4w9WgXc!",       // bad char
            "https://youtu.be/",                  // empty
            "https://youtu.be/dQw4w9WgXcQ/extra", // extra path segment
            "https://www.youtube.com/shorts/",    // missing id
            // non-YouTube
            "https://vimeo.com/123456789",
            "https://www.evil-youtube.com/watch?v=dQw4w9WgXcQ",
            "https://youtube.com.evil.test/watch?v=dQw4w9WgXcQ",
            "not a url at all",
        ];
        for url in cases {
            assert!(detect_youtube(url).is_none(), "expected miss: {url}");
        }
    }

    // ─── Error semantics ─────────────────────────────────────────────────

    #[test]
    fn soft_vs_hard_classification() {
        assert!(YoutubeError::YtDlpUnavailable.is_soft());
        assert!(YoutubeError::NoSubtitles.is_soft());
        assert!(!YoutubeError::FetchFailed("x".into()).is_soft());
        assert!(!YoutubeError::ParseFailed("x".into()).is_soft());
    }

    // ─── VTT cleaning ────────────────────────────────────────────────────

    /// Classic YouTube auto-caption shape: header metadata, inline timing
    /// tags, and the rolling 2-line window where each cue repeats the
    /// previous cue's tail.
    const ROLLING_SAMPLE: &str = "WEBVTT
Kind: captions
Language: en

00:00:00.000 --> 00:00:01.500 align:start position:0%
hello<00:00:00.500><c> world</c>

00:00:01.000 --> 00:00:02.500 align:start position:0%
hello world
this<00:00:01.500><c> is</c><00:00:02.000><c> a</c>

00:00:02.000 --> 00:00:03.500 align:start position:0%
this is a
test

00:00:03.000 --> 00:00:04.500 align:start position:0%
this is a
test
";

    #[test]
    fn rolling_overlap_is_deduped() {
        let text = clean_vtt(ROLLING_SAMPLE);
        assert_eq!(text, "hello world this is a test");
    }

    #[test]
    fn cleaning_at_least_halves_rolling_auto_subs() {
        // Synthesize a long rolling transcript: cue i shows lines
        // [i-1, i], so every line past the first appears twice in raw form.
        let mut raw = String::from("WEBVTT\n\n");
        for i in 0..60 {
            let start = i as f64;
            raw.push_str(&format!(
                "{:02}:{:02}.000 --> {:02}:{:02}.000\n",
                (start / 60.0) as u32,
                start % 60.0,
                ((start + 2.0) / 60.0) as u32,
                (start + 2.0) % 60.0,
            ));
            if i > 0 {
                // Zero-padded so substring matching below stays unique.
                raw.push_str(&format!("line {:03}\n", i - 1));
            }
            raw.push_str(&format!("line {i:03}\n\n"));
        }
        let cleaned = clean_vtt(&raw);
        assert!(
            cleaned.len() * 2 < raw.len(),
            "expected ≥2x reduction: raw {} chars, cleaned {} chars",
            raw.len(),
            cleaned.len()
        );
        // And every line survives exactly once, in order.
        for i in 0..60 {
            assert_eq!(cleaned.matches(&format!("line {i:03}")).count(), 1);
        }
    }

    #[test]
    fn inline_tags_and_entities_are_stripped() {
        let vtt = "WEBVTT\n\n00:00:01.000 --> 00:00:03.000\n<c.colorE5E5E5>Tom &amp; Jerry</c> said &quot;hi&quot; &#39;twice&nbsp;&gt;\n\n";
        let text = clean_vtt(vtt);
        assert_eq!(text, "Tom & Jerry said \"hi\" 'twice >");
        assert!(!text.contains('<'), "tag residue: {text}");
    }

    #[test]
    fn empty_cues_notes_and_headers_are_dropped() {
        let vtt = "WEBVTT
Kind: captions
Language: en

NOTE this is a comment block
spanning two lines

00:00:01.000 --> 00:00:02.000

cue-identifier-42
00:00:02.000 --> 00:00:03.500
only text

00:00:10.000 --> 00:00:11.000
   \n\n";
        let text = clean_vtt(vtt);
        assert_eq!(text, "only text");
    }

    #[test]
    fn adjacent_duplicate_lines_collapse() {
        let vtt = "WEBVTT\n\n00:00:00.000 --> 00:00:01.000\nsame line\nsame line\n\n00:00:01.000 --> 00:00:02.000\nsame line\nnext line\n\n";
        assert_eq!(clean_vtt(vtt), "same line next line");
    }

    #[test]
    fn silence_gap_creates_paragraph_break() {
        let vtt = "WEBVTT\n\n00:00:00.000 --> 00:00:01.000\nfirst thought\n\n00:00:05.000 --> 00:00:06.000\nsecond thought\n\n";
        assert_eq!(clean_vtt(vtt), "first thought\n\nsecond thought");
    }

    #[test]
    fn clean_vtt_is_lenient_without_header() {
        // Pure function: no header requirement (the fetch path enforces it).
        assert_eq!(
            clean_vtt("00:00:00.000 --> 00:00:01.000\nbare cue\n"),
            "bare cue"
        );
    }

    // ─── Full pipeline via fake runner ───────────────────────────────────

    /// Fake [`YtDlpRunner`] returning a canned result and capturing args.
    struct FakeRunner {
        result: Mutex<Option<std::result::Result<ProcessOutput, YoutubeError>>>,
        seen_args: Mutex<Vec<String>>,
    }

    impl FakeRunner {
        fn ok(stdout: &str) -> Self {
            Self::with_result(Ok(ProcessOutput {
                success: true,
                code: Some(0),
                stdout: stdout.to_string(),
            }))
        }

        fn with_result(result: std::result::Result<ProcessOutput, YoutubeError>) -> Self {
            Self {
                result: Mutex::new(Some(result)),
                seen_args: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl YtDlpRunner for FakeRunner {
        async fn run(&self, args: &[String]) -> std::result::Result<ProcessOutput, YoutubeError> {
            self.seen_args.lock().unwrap().extend(args.iter().cloned());
            self.result
                .lock()
                .unwrap()
                .take()
                .expect("FakeRunner called more than once")
        }
    }

    fn target() -> YouTubeTarget {
        detect_youtube("https://youtu.be/dQw4w9WgXcQ").unwrap()
    }

    #[tokio::test]
    async fn pipeline_success_cleans_transcript() {
        let runner = FakeRunner::ok(ROLLING_SAMPLE);
        let transcript = fetch_transcript_with(&runner, &target()).await.unwrap();
        assert_eq!(transcript.text(), "hello world this is a test");

        let args = runner.seen_args.lock().unwrap();
        assert!(args.contains(&"--skip-download".to_string()));
        assert!(args.contains(&"--write-auto-subs".to_string()));
        assert_eq!(
            args.last().map(String::as_str),
            Some("https://www.youtube.com/watch?v=dQw4w9WgXcQ")
        );
    }

    #[tokio::test]
    async fn pipeline_propagates_ytdlp_unavailable_as_soft() {
        let runner = FakeRunner::with_result(Err(YoutubeError::YtDlpUnavailable));
        let err = fetch_transcript_with(&runner, &target()).await.unwrap_err();
        assert_eq!(err, YoutubeError::YtDlpUnavailable);
        assert!(err.is_soft());
    }

    #[tokio::test]
    async fn pipeline_empty_stdout_means_no_subtitles_soft() {
        let runner = FakeRunner::ok("");
        let err = fetch_transcript_with(&runner, &target()).await.unwrap_err();
        assert_eq!(err, YoutubeError::NoSubtitles);
        assert!(err.is_soft());
    }

    #[tokio::test]
    async fn pipeline_nonzero_exit_is_hard_fetch_failure() {
        let runner = FakeRunner::with_result(Ok(ProcessOutput {
            success: false,
            code: Some(1),
            stdout: String::new(),
        }));
        let err = fetch_transcript_with(&runner, &target()).await.unwrap_err();
        assert!(!err.is_soft());
        match err {
            YoutubeError::FetchFailed(msg) => {
                assert!(msg.contains("code 1"), "exit code missing: {msg}");
            }
            other => panic!("expected FetchFailed, got {other:?}"),
        }
    }

    /// The real runner must not let stderr (URLs, cookie paths) cross the
    /// boundary. Drives a real child that writes a marker to stderr and
    /// exits non-zero: the returned `ProcessOutput` has no stderr channel
    /// at all, so containment is structural, and stdout must not pick up
    /// anything the child wrote to stderr.
    #[tokio::test]
    #[cfg(unix)]
    async fn real_runner_stderr_never_crosses_boundary() {
        let runner = YtDlpCommand::with_binary(PathBuf::from("/bin/sh"), Duration::from_secs(5));
        let args = vec![
            "-c".to_string(),
            "echo 'cookie file /home/u/.config/cookies.txt for https://x.test/secret' >&2; exit 1"
                .to_string(),
        ];
        let out = runner.run(&args).await.unwrap();
        assert!(!out.success);
        assert_eq!(out.code, Some(1));
        assert!(out.stdout.is_empty(), "stderr leaked into stdout");
    }

    #[tokio::test]
    async fn pipeline_oversized_output_is_rejected() {
        let big = format!("WEBVTT\n\n{}", "x".repeat(MAX_SUBTITLE_BYTES + 1));
        let runner = FakeRunner::ok(&big);
        let err = fetch_transcript_with(&runner, &target()).await.unwrap_err();
        assert!(matches!(err, YoutubeError::FetchFailed(_)));
    }

    #[tokio::test]
    async fn pipeline_non_vtt_stdout_is_parse_failure() {
        let runner = FakeRunner::ok("<html>not subtitles</html>");
        let err = fetch_transcript_with(&runner, &target()).await.unwrap_err();
        assert!(matches!(err, YoutubeError::ParseFailed(_)));
    }

    #[tokio::test]
    async fn pipeline_vtt_with_only_empty_cues_means_no_subtitles() {
        let runner = FakeRunner::ok("WEBVTT\n\n00:00:00.000 --> 00:00:01.000\n\n");
        let err = fetch_transcript_with(&runner, &target()).await.unwrap_err();
        assert_eq!(err, YoutubeError::NoSubtitles);
    }

    #[tokio::test]
    async fn pipeline_concatenated_vtt_documents_keep_first() {
        let two_docs = "WEBVTT\n\n00:00:00.000 --> 00:00:01.000\nenglish line\n\nWEBVTT\n\n00:00:00.000 --> 00:00:01.000\nchinese line\n";
        let runner = FakeRunner::ok(two_docs);
        let transcript = fetch_transcript_with(&runner, &target()).await.unwrap();
        assert_eq!(transcript.text(), "english line");
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn real_runner_timeout_kills_child() {
        // /bin/sleep ignores yt-dlp-style args poorly, so drive the runner
        // directly: sleep 5 with a 50ms budget must time out and the child
        // must be reaped (kill_on_drop).
        let runner =
            YtDlpCommand::with_binary(PathBuf::from("/bin/sleep"), Duration::from_millis(50));
        let err = runner.run(&["5".to_string()]).await.unwrap_err();
        match err {
            YoutubeError::FetchFailed(msg) => {
                assert!(msg.contains("timed out"), "got: {msg}");
            }
            other => panic!("expected timeout FetchFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn real_runner_missing_binary_maps_to_unavailable() {
        let runner = YtDlpCommand::with_binary(
            PathBuf::from("/nonexistent/yt-dlp-definitely-missing"),
            Duration::from_secs(1),
        );
        let err = runner.run(&[]).await.unwrap_err();
        assert_eq!(err, YoutubeError::YtDlpUnavailable);
        assert!(err.is_soft());
    }

    #[test]
    fn ytdlp_args_shape() {
        let args = ytdlp_args("https://www.youtube.com/watch?v=dQw4w9WgXcQ");
        assert!(args.windows(2).any(|w| w[0] == "-o" && w[1] == "-"));
        assert!(args.contains(&"--sub-format".to_string()));
        assert!(args.contains(&"vtt/best".to_string()));
        // URL is last and protected by `--`.
        let dashdash = args.iter().position(|a| a == "--").unwrap();
        assert_eq!(dashdash, args.len() - 2);
    }
}
