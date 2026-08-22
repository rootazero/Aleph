//! `media/codecs` — can this machine actually decode what the Panel plays?
//!
//! Linux-only. WebKitGTK plays media through GStreamer, and MP3/AAC decoding
//! lives in `gstreamer1.0-plugins-{bad,ugly}`, which many distributions do not
//! install by default. When they are absent the Panel's TTS playback rejects
//! silently: the user hears nothing and no surface says which package is
//! missing.
//!
//! # Why GStreamer is asked directly
//!
//! Not by querying distro package names — those hold under neither Flatpak,
//! Snap, nor a source build. `gst-inspect-1.0 --exists <element>` asks the
//! registry that WebKitGTK itself will consult.
//!
//! # Three states, not two
//!
//! `gst-inspect-1.0` may itself be absent (`gstreamer1.0-tools`). That answer
//! is "I don't know", and it is reported as such: it must not be read as
//! healthy, and equally must not be read as broken (CLAUDE.md §8). Severity
//! alone cannot carry that distinction — `Unknown` and `Ok` are both
//! `Severity::Info`, and every machine consumer (the `--json` lint surface,
//! CI, the LLM `doctor` tool) reads severity, not the English prose in the
//! title. So each verdict also carries its own machine-readable tag
//! (`TAG_CODECS_*` below) — the same pattern `providers_connectivity` and
//! `sqlite_integrity` use to separate machine-readable states inside one
//! severity.

use std::time::Duration;

use async_trait::async_trait;

use crate::diagnostics::check::{HealthCheck, Posture, DEFAULT_CHECK_TIMEOUT};
use crate::diagnostics::finding::Finding;
#[cfg(any(target_os = "linux", test))]
use crate::diagnostics::finding::Severity;

const CHECK_ID: &str = "media/codecs";

// `Format`, `FORMATS`, `CodecVerdict`, `findings_for` and the `TAG_CODECS_*`
// consts below are consumed only by the Linux probe path (`probe`/`run`) and
// by the tests that prove the verdict-to-finding mapping. On Windows/macOS
// the non-Linux `run` arm (bottom of this file) returns `Vec::new()` without
// touching any of them, so without this gate the non-test lib target on
// those platforms would carry permanent dead-code warnings.
/// One decodable format and the GStreamer elements that can provide it.
///
/// Alternatives are OR-ed: `avdec_mp3` (libav) and `mpg123audiodec` both
/// decode MP3, and either is enough.
#[cfg(any(target_os = "linux", test))]
struct Format {
    label: &'static str,
    elements: &'static [&'static str],
    package_hint: &'static str,
}

#[cfg(any(target_os = "linux", test))]
const FORMATS: &[Format] = &[
    Format {
        label: "MP3",
        elements: &["mpg123audiodec", "avdec_mp3"],
        package_hint: "gstreamer1.0-plugins-ugly (or gstreamer1.0-libav)",
    },
    Format {
        label: "AAC",
        elements: &["avdec_aac", "faad"],
        package_hint: "gstreamer1.0-plugins-bad (or gstreamer1.0-libav)",
    },
    Format {
        label: "Opus",
        elements: &["opusdec"],
        package_hint: "gstreamer1.0-plugins-base",
    },
    Format {
        label: "VP8/VP9 (WebM)",
        elements: &["vp8dec", "vp9dec"],
        package_hint: "gstreamer1.0-plugins-good",
    },
];

/// Tag on the `Ok` verdict's finding — every format has at least one decoder.
#[cfg(any(target_os = "linux", test))]
const TAG_CODECS_OK: &str = "codecs-ok";
/// Tag on the `Missing` verdict's finding — at least one format has none.
#[cfg(any(target_os = "linux", test))]
const TAG_CODECS_MISSING: &str = "codecs-missing";
/// Tag on the `Unknown` verdict's finding. This exists because severity alone
/// cannot separate "I don't know" from "it's fine" — both are
/// `Severity::Info` — and prose in the title cannot be read by CI or the
/// `--json` lint surface; only a machine-readable tag can.
#[cfg(any(target_os = "linux", test))]
const TAG_CODECS_UNKNOWN: &str = "codecs-unknown";

/// What the probe learned.
#[cfg(any(target_os = "linux", test))]
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CodecVerdict {
    /// Every format has at least one decoder.
    Ok,
    /// These formats have no decoder. Non-empty by construction.
    Missing(Vec<String>),
    /// The probe could not run. NOT healthy and NOT broken.
    Unknown(String),
}

/// Turn a verdict into findings. Pure, so the mapping is testable without a
/// GStreamer installation.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn findings_for(verdict: &CodecVerdict) -> Vec<Finding> {
    match verdict {
        CodecVerdict::Ok => vec![Finding::ok(
            CHECK_ID,
            "Media decoders present",
            "GStreamer can decode every format the Panel plays.",
        )
        .with_tag(TAG_CODECS_OK)],
        CodecVerdict::Missing(formats) => vec![Finding::problem(
            CHECK_ID,
            Severity::Warning,
            "Missing media decoders",
            format!(
                "GStreamer has no decoder for: {}. Voice replies and media \
                 attachments in these formats will fail to play in the Panel, \
                 and the failure is silent at the WebKitGTK layer.",
                formats.join(", ")
            ),
        )
        .with_fix_hint(
            FORMATS
                .iter()
                .filter(|f| formats.iter().any(|m| m.starts_with(f.label)))
                .map(|f| format!("{}: install {}", f.label, f.package_hint))
                .collect::<Vec<_>>()
                .join("; "),
        )
        .with_tag(TAG_CODECS_MISSING)],
        // Deliberately Severity::Info, same as `Ok` — the three-state
        // distinction lives in the tag (`TAG_CODECS_UNKNOWN`), not in
        // severity. Severity is the one field every machine consumer reads
        // (the --json lint surface, CI, the LLM doctor tool), and
        // Info-vs-Info carries no signal by itself; the tag is what lets
        // "I don't know" stay distinguishable from "it's fine" downstream.
        CodecVerdict::Unknown(reason) => vec![Finding::problem(
            CHECK_ID,
            Severity::Info,
            "Media decoder status unknown",
            format!("Could not determine which media formats this system can decode: {reason}"),
        )
        .with_fix_hint("Install gstreamer1.0-tools to let `aleph doctor` answer this.")
        .with_tag(TAG_CODECS_UNKNOWN)],
    }
}

/// Linux media-decoder availability.
pub struct MediaCodecsCheck;

impl MediaCodecsCheck {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for MediaCodecsCheck {
    fn default() -> Self {
        Self::new()
    }
}

/// Ask GStreamer whether one element exists.
#[cfg(target_os = "linux")]
async fn element_exists(name: &str) -> Result<bool, std::io::Error> {
    // tokio::process, not spawn_blocking: the engine wraps this check in
    // tokio::time::timeout, and only a genuinely cancellable child honours it.
    // A cold `gst-inspect-1.0` rebuilds the registry and can take seconds.
    let status = tokio::process::Command::new("gst-inspect-1.0")
        .arg("--exists")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await?;
    Ok(status.success())
}

#[cfg(target_os = "linux")]
async fn probe() -> CodecVerdict {
    let mut missing = Vec::new();
    for format in FORMATS {
        let mut have = false;
        for element in format.elements {
            match element_exists(element).await {
                Ok(true) => {
                    have = true;
                    break;
                }
                Ok(false) => {}
                Err(e) => {
                    // The tool itself is unavailable — the answer for EVERY
                    // format is "unknown", not "missing". Reporting the first
                    // format as missing would be a confident wrong answer.
                    return CodecVerdict::Unknown(format!(
                        "gst-inspect-1.0 could not be run ({e}); install gstreamer1.0-tools"
                    ));
                }
            }
        }
        if !have {
            missing.push(format.label.to_string());
        }
    }
    if missing.is_empty() {
        CodecVerdict::Ok
    } else {
        CodecVerdict::Missing(missing)
    }
}

#[async_trait]
impl HealthCheck for MediaCodecsCheck {
    fn id(&self) -> &'static str {
        CHECK_ID
    }

    fn title(&self) -> &'static str {
        "Media decoders"
    }

    /// Inner bound: four formats x up to two `gst-inspect-1.0` invocations. A
    /// cold run rebuilds the GStreamer registry once (seconds); every later
    /// invocation reads the cache. The default 20s ceiling covers that with
    /// room, and the engine turns an overrun into a named Warning rather than
    /// a stall inside an agent turn.
    fn timeout(&self) -> Duration {
        DEFAULT_CHECK_TIMEOUT
    }

    #[cfg(target_os = "linux")]
    async fn run(&self, _posture: Posture) -> Vec<Finding> {
        findings_for(&probe().await)
    }

    /// Windows (WebView2) and macOS (WKWebView) decode media through the OS,
    /// with no separate plugin set to be missing. Nothing to report.
    #[cfg(not(target_os = "linux"))]
    async fn run(&self, _posture: Posture) -> Vec<Finding> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Guards the static table itself: an empty `elements` list would make
    /// `probe`'s inner loop never set `have = true`, so that format would be
    /// reported `Missing` unconditionally regardless of what is actually
    /// installed. `probe` only runs on Linux, so this is the only place that
    /// reads `Format::elements` on other platforms.
    #[test]
    fn every_format_lists_at_least_one_element() {
        for format in FORMATS {
            assert!(
                !format.elements.is_empty(),
                "{} format has no elements to probe",
                format.label
            );
        }
    }

    #[test]
    fn ok_reports_a_single_info_finding() {
        let f = findings_for(&CodecVerdict::Ok);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Info);
        assert!(f[0].has_tag(TAG_CODECS_OK));
    }

    #[test]
    fn missing_names_the_formats_and_the_packages() {
        let f = findings_for(&CodecVerdict::Missing(vec!["MP3".into(), "AAC".into()]));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Warning);
        assert!(f[0].has_tag(TAG_CODECS_MISSING));
        assert!(f[0].detail.contains("MP3"));
        assert!(f[0].detail.contains("AAC"));
        let hint = f[0].fix_hint.as_deref().unwrap_or_default();
        assert!(
            hint.contains("gstreamer1.0-plugins-ugly"),
            "hint was: {hint}"
        );
        assert!(
            hint.contains("gstreamer1.0-plugins-bad"),
            "hint was: {hint}"
        );
    }

    /// The whole point of the third state: "I could not tell" must never be
    /// rendered as either health or breakage. Severity can't carry that on
    /// its own — `Unknown` and `Ok` are both `Severity::Info` — so the tag
    /// assertion below is the load-bearing one; the title/detail assertions
    /// are kept only as a secondary, human-facing check.
    #[test]
    fn unknown_is_neither_ok_nor_a_warning() {
        let f = findings_for(&CodecVerdict::Unknown("tool absent".into()));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Info);
        assert!(
            f[0].has_tag(TAG_CODECS_UNKNOWN),
            "an unknown verdict must carry a tag distinct from Ok — severity \
             alone reads the same as health to every machine consumer \
             (--json, CI, the LLM doctor tool)"
        );
        assert!(f[0].title.contains("unknown"), "title was: {}", f[0].title);
        assert!(
            f[0].detail.contains("Could not determine"),
            "an unknown verdict must SAY it is unknown, not imply health"
        );
    }

    /// Windows (WebView2) and macOS (WKWebView) decode through the OS, so
    /// this check must contribute nothing there — not a finding saying
    /// "not applicable", which would be noise in every report on two of the
    /// three platforms.
    #[cfg(not(target_os = "linux"))]
    #[tokio::test]
    async fn the_check_is_inert_off_linux() {
        assert!(MediaCodecsCheck::new()
            .run(Posture::Inspect)
            .await
            .is_empty());
    }
}
