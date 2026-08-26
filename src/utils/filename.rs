//! One sanitizer for caller-supplied filenames.
//!
//! Two surfaces used to carry a private `sanitize_filename` of the same name
//! and the same stated purpose, with different strength — whichever got
//! hardened next, the other silently stayed behind. They now share this one.

use std::path::Path;

use super::paths::is_windows_reserved_name;

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
    // Windows reserves a small set of device names (CON, PRN, AUX, NUL,
    // COM1-9, LPT1-9) at the filesystem layer — case-insensitively and on the
    // STEM, which means "CON.txt" is reserved as well: Win32 drops the
    // extension when resolving a device, so that name opens the console rather
    // than creating a file. A sanitized name that lands on one of these would
    // either fail to create or, worse, redirect a write to the device. Map them
    // to the fallback like every other unrecoverable shape.
    if trimmed.is_empty() || is_windows_reserved_name(trimmed) {
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

    /// Windows reserves a small set of device names at the FS layer regardless
    /// of extension. A sanitized name landing on one would either fail to
    /// create or, worse, redirect to a device — both silently for a caller that
    /// only sees the returned string.
    #[test]
    fn windows_reserved_device_names_become_the_fallback() {
        for raw in ["CON", "con", "Con", "PRN", "AUX", "NUL", "COM1", "LPT9"] {
            assert_eq!(
                sanitize_filename(raw),
                FALLBACK_FILENAME,
                "{raw:?} must map to the fallback"
            );
        }
        // The reserved set applies to the STEM, and an extension does not
        // rescue it: Win32 resolves `CON.txt` to the console device, so a
        // "file" written under that name goes to the device. This pair used to
        // assert the opposite — the misconception travelled from the SSOT's doc
        // comment (`utils::paths::is_windows_reserved_name`) into a test, where
        // it then read as a deliberate carve-out rather than an error.
        for raw in ["CON.txt", "con.log", "NUL.md", "LPT9.tar"] {
            assert_eq!(
                sanitize_filename(raw),
                FALLBACK_FILENAME,
                "{raw:?} resolves to a device on Windows and must map to the fallback"
            );
        }
        // Formerly a recorded gap: the stem used to be taken at the LAST dot,
        // so `con.tar.gz` yielded the stem `con.tar` and passed while Win32
        // resolved it to the console device. The SSOT now cuts at the FIRST
        // dot. Multi-extension names are the common shape for exactly the
        // files this matters for — an archive a tool writes under a
        // model-supplied name.
        for raw in ["con.tar.gz", "CON.tar.gz", "nul.a.b.c", "aux.h"] {
            assert_eq!(
                sanitize_filename(raw),
                FALLBACK_FILENAME,
                "{raw:?} resolves to a device on Windows and must map to the fallback"
            );
        }
        // The SSOT also cuts at `:` and ignores trailing spaces, because the
        // DOS-device parser does. Both are no-ops *here* — `:` is filtered out
        // and trailing spaces are trimmed before the check runs — so these
        // assert the sanitizer's own two steps still land first, not the
        // stem rule. `con:foo` losing its colon leaves the ordinary name
        // `confoo`; it must not become the fallback.
        assert_eq!(sanitize_filename("con:foo"), "confoo");
        assert_eq!(sanitize_filename("con "), FALLBACK_FILENAME);
        // A name that merely STARTS with a reserved word is a normal file —
        // the check is equality on the stem, not a prefix match.
        assert_eq!(sanitize_filename("CONTACTS.txt"), "CONTACTS.txt");
        assert_eq!(sanitize_filename("console.log"), "console.log");
    }
}
