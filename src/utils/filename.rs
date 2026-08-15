//! One sanitizer for caller-supplied filenames.
//!
//! Two surfaces used to carry a private `sanitize_filename` of the same name
//! and the same stated purpose, with different strength — whichever got
//! hardened next, the other silently stayed behind. They now share this one.

use std::path::Path;

/// Longest sanitized filename kept, in characters. Keeps the value comfortably
/// under filesystem and `Content-Disposition` limits downstream.
pub(crate) const MAX_FILENAME_CHARS: usize = 200;

/// Filename used when the caller's name sanitizes down to nothing.
///
/// Load-bearing on both call sites: the artifact store records it as the
/// display name, and the media cache joins it into a temp path, so it must
/// stay a non-empty, separator-free single component.
pub(crate) const FALLBACK_FILENAME: &str = "unnamed";

/// Reduce a caller-supplied name to a plain display filename: no directory
/// components, no characters that are illegal on Windows, no control bytes,
/// bounded length, never empty.
///
/// # The property callers rely on
///
/// **A directory path can never survive this.** The result is always exactly
/// one path component: only [`Path::file_name`] escapes, and the separators
/// `/` and `\` are stripped from what is left, so joining the result onto a
/// directory can never address anything outside that directory. `..` and a
/// name that trims to nothing both become [`FALLBACK_FILENAME`], so the result
/// is also never empty and never the parent link.
///
/// # Blast radius
///
/// Two call sites depend on the above; harden with both in view.
///
/// - [`crate::artifacts::store::ArtifactStore::put`] — the value becomes the
///   record's *display* `filename` (the on-disk blob is id-addressed, so this
///   never names a file), and is echoed to clients in a download name.
/// - `crate::media::cache::unique_filename` — the value becomes an *actual*
///   path component under `<private_temp_root>/media/<session>/` (the 0700
///   per-uid root from [`crate::utils::paths::private_temp_root`]), prefixed
///   with a per-item id so parallel downloads of the same display name cannot
///   collide.
///
/// Deriving a filename from a *title* rather than from a path is a different
/// concept with a different function — see `artifact_publish::slug_filename`.
pub(crate) fn sanitize_filename(name: &str) -> String {
    let base = Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();

    let cleaned: String = base
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(c, '/' | '\\' | ':' | '"' | '<' | '>' | '|' | '?' | '*')
        })
        .take(MAX_FILENAME_CHARS)
        .collect();

    // Trailing dots and spaces are silently dropped by Windows; strip them here
    // so the recorded name matches what any filesystem would keep. This also
    // turns a residual ".." into the fallback.
    let trimmed = cleaned.trim().trim_end_matches('.').trim_end();
    if trimmed.is_empty() {
        FALLBACK_FILENAME.to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one property every caller relies on: whatever goes in, exactly one
    /// path component comes out, and it is never `..` and never empty.
    #[test]
    fn no_directory_path_survives() {
        for raw in [
            "../../etc/passwd",
            "/etc/passwd",
            "foo/bar.txt",
            "a\\b\\c.txt",
            "..",
            ".",
            "",
            "   ",
            "....",
            "./../.././secret",
        ] {
            let out = sanitize_filename(raw);
            assert!(!out.is_empty(), "{raw:?} sanitized to an empty name");
            assert_ne!(out, "..", "{raw:?} sanitized to the parent link");
            assert!(
                !out.contains('/') && !out.contains('\\'),
                "{raw:?} kept a separator: {out:?}"
            );
            assert_eq!(
                Path::new(&out).components().count(),
                1,
                "{raw:?} sanitized to more than one component: {out:?}"
            );
        }

        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("foo/bar.txt"), "bar.txt");
        assert_eq!(sanitize_filename(".."), FALLBACK_FILENAME);
        assert_eq!(sanitize_filename(""), FALLBACK_FILENAME);
    }

    #[test]
    fn strips_control_bytes_and_characters_illegal_on_windows() {
        let out = sanitize_filename("re\u{7}port:\"<>|?*.txt\r\n");
        assert_eq!(out, "report.txt");
    }

    #[test]
    fn trailing_dots_and_spaces_are_dropped() {
        assert_eq!(sanitize_filename("report.txt.  "), "report.txt");
        assert_eq!(sanitize_filename("  spaced.txt  "), "spaced.txt");
    }

    #[test]
    fn sanitize_filename_is_utf8_safe_when_truncating() {
        let name = "é".repeat(MAX_FILENAME_CHARS + 50);
        assert_eq!(sanitize_filename(&name).chars().count(), MAX_FILENAME_CHARS);
    }

    #[test]
    fn a_plain_name_passes_through_untouched() {
        assert_eq!(sanitize_filename("chart.png"), "chart.png");
        assert_eq!(sanitize_filename("季度复盘.md"), "季度复盘.md");
    }
}
