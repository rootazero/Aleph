//! Prompt Sanitization
//!
//! Prevents prompt injection by sanitizing untrusted content before
//! embedding in system prompts. Three levels of sanitization for
//! different trust levels.

/// Sanitization strength level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SanitizeLevel {
    /// Paths, environment variables — strip ALL control and format characters.
    Strict,
    /// User instructions, workspace files — preserve newlines/tabs, strip other control chars.
    Moderate,
    /// Internal generated text — only strip injection markers.
    Light,
}

/// Sanitize a string for safe embedding in a prompt.
///
/// # Levels
///
/// - **Strict**: Strip ALL control chars (`is_control()`) AND Unicode format chars.
///   No newlines, no tabs, nothing. Suitable for paths and environment variables.
/// - **Moderate**: Keep `\n`, `\t`, `\r`. Strip all other control/format chars.
///   Suitable for user instructions and workspace files.
/// - **Light**: Only strip injection markers (`<system>`, `<system-reminder>`, etc.).
///   Pass everything else through. Suitable for internal generated text.
pub fn sanitize_for_prompt(value: &str, level: SanitizeLevel) -> String {
    match level {
        SanitizeLevel::Strict => {
            value
                .chars()
                .filter(|c| !c.is_control() && !is_format_char(*c))
                .collect()
        }
        SanitizeLevel::Moderate => {
            value
                .chars()
                .filter(|c| {
                    // Preserve whitespace chars we want to keep
                    if *c == '\n' || *c == '\t' || *c == '\r' {
                        return true;
                    }
                    // Strip all other control and format chars
                    !c.is_control() && !is_format_char(*c)
                })
                .collect()
        }
        SanitizeLevel::Light => strip_injection_markers(value),
    }
}

/// Check for Unicode format characters (category Cf) and line/paragraph separators.
///
/// Includes:
/// - Unicode Cf (format) characters: zero-width spaces, joiners, direction marks, etc.
/// - U+2028 (Line Separator) and U+2029 (Paragraph Separator)
fn is_format_char(c: char) -> bool {
    // U+2028 and U+2029 are line/paragraph separators (category Zl/Zp)
    if c == '\u{2028}' || c == '\u{2029}' {
        return true;
    }

    // Check Unicode general category Cf (format characters)
    // Common Cf characters that appear in prompt injection attempts:
    // U+200B Zero Width Space
    // U+200C Zero Width Non-Joiner
    // U+200D Zero Width Joiner
    // U+200E Left-to-Right Mark
    // U+200F Right-to-Left Mark
    // U+FEFF Byte Order Mark / Zero Width No-Break Space
    // U+00AD Soft Hyphen
    // U+061C Arabic Letter Mark
    // U+2060-U+2064 Invisible operators
    // U+2066-U+2069 Bidirectional isolates
    // U+206A-U+206F Deprecated format chars
    //
    // Rather than enumerate all, we use a heuristic that covers the known ranges.
    // Rust's char doesn't expose the Unicode general category directly without
    // a dependency, so we check the known Cf ranges.
    matches!(c,
        '\u{00AD}' |           // Soft Hyphen
        '\u{061C}' |           // Arabic Letter Mark
        '\u{070F}' |           // Syriac Abbreviation Mark
        '\u{0890}'..='\u{0891}' | // Arabic Pound/Piastre Mark
        '\u{08E2}' |           // Arabic Disputed End of Ayah
        '\u{180E}' |           // Mongolian Vowel Separator
        '\u{200B}'..='\u{200F}' | // Zero-width and direction marks
        '\u{202A}'..='\u{202E}' | // Bidirectional formatting
        '\u{2060}'..='\u{2064}' | // Invisible operators
        '\u{2066}'..='\u{206F}' | // Bidi isolates + deprecated
        '\u{FEFF}' |           // BOM / ZWNBSP
        '\u{FFF9}'..='\u{FFFB}' | // Interlinear annotation anchors
        '\u{110BD}' |          // Kaithi Number Sign
        '\u{110CD}' |          // Kaithi Number Sign Above
        '\u{13430}'..='\u{1343F}' | // Egyptian Hieroglyph format chars
        '\u{1BCA0}'..='\u{1BCA3}' | // Shorthand format controls
        '\u{1D173}'..='\u{1D17A}' | // Musical symbol format chars
        '\u{E0001}' |          // Language Tag
        '\u{E0020}'..='\u{E007F}'  // Tag components
    )
}

/// Strip known injection marker tags from the input.
///
/// Removes: `<system-reminder>`, `</system-reminder>`, `<system>`, `</system>`
fn strip_injection_markers(value: &str) -> String {
    const MARKERS: &[&str] = &[
        "<system-reminder>",
        "</system-reminder>",
        "<system>",
        "</system>",
    ];

    let mut result = value.to_string();
    for marker in MARKERS {
        result = result.replace(marker, "");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strict_strips_all_control_chars() {
        let input = "hello\x00world\x07\x1b[31mred";
        let result = sanitize_for_prompt(input, SanitizeLevel::Strict);
        assert_eq!(result, "helloworld[31mred");
    }

    #[test]
    fn test_strict_strips_newlines() {
        let input = "line1\nline2\rline3";
        let result = sanitize_for_prompt(input, SanitizeLevel::Strict);
        assert_eq!(result, "line1line2line3");
    }

    #[test]
    fn test_strict_strips_format_chars() {
        let input = "hello\u{200B}world\u{200D}test";
        let result = sanitize_for_prompt(input, SanitizeLevel::Strict);
        assert_eq!(result, "helloworldtest");
    }

    #[test]
    fn test_strict_strips_line_separators() {
        let input = "hello\u{2028}world\u{2029}test";
        let result = sanitize_for_prompt(input, SanitizeLevel::Strict);
        assert_eq!(result, "helloworldtest");
    }

    #[test]
    fn test_moderate_preserves_newlines_and_tabs() {
        let input = "line1\nline2\ttab";
        let result = sanitize_for_prompt(input, SanitizeLevel::Moderate);
        assert_eq!(result, "line1\nline2\ttab");
    }

    #[test]
    fn test_moderate_strips_other_control_chars() {
        let input = "hello\x00\x07world";
        let result = sanitize_for_prompt(input, SanitizeLevel::Moderate);
        assert_eq!(result, "helloworld");
    }

    #[test]
    fn test_light_strips_injection_markers() {
        let input = "normal text <system-reminder>injected</system-reminder> more text";
        let result = sanitize_for_prompt(input, SanitizeLevel::Light);
        assert_eq!(result, "normal text injected more text");
    }

    #[test]
    fn test_light_strips_system_tags() {
        let input = "text <system>evil</system> end";
        let result = sanitize_for_prompt(input, SanitizeLevel::Light);
        assert_eq!(result, "text evil end");
    }

    #[test]
    fn test_light_preserves_all_other_content() {
        let input = "hello\nworld\t\x00\u{200B}";
        let result = sanitize_for_prompt(input, SanitizeLevel::Light);
        assert_eq!(result, "hello\nworld\t\x00\u{200B}");
    }

    #[test]
    fn test_empty_string() {
        assert_eq!(sanitize_for_prompt("", SanitizeLevel::Strict), "");
        assert_eq!(sanitize_for_prompt("", SanitizeLevel::Moderate), "");
        assert_eq!(sanitize_for_prompt("", SanitizeLevel::Light), "");
    }

    #[test]
    fn test_ascii_only_passes_through() {
        let input = "Hello, World! 123 #@$%";
        assert_eq!(sanitize_for_prompt(input, SanitizeLevel::Strict), input);
        assert_eq!(sanitize_for_prompt(input, SanitizeLevel::Moderate), input);
        assert_eq!(sanitize_for_prompt(input, SanitizeLevel::Light), input);
    }
}
