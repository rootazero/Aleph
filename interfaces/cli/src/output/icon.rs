//! Status icons with Unicode + ASCII fallback.
//!
//! Detection is cached via `OnceLock`. Unicode is emitted unless:
//! - `ALEPH_ASCII=1` (or `true`)
//! - locale is C/POSIX
//!
//! Each constant function returns the appropriate glyph for the current
//! terminal; callers should NOT cache the result themselves.

use std::sync::OnceLock;

/// Whether to emit Unicode glyphs vs ASCII fallback.
pub fn use_unicode() -> bool {
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(detect_unicode)
}

fn detect_unicode() -> bool {
    if let Ok(v) = std::env::var("ALEPH_ASCII") {
        match v.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" => return false,
            "0" | "false" | "no" => return true,
            _ => {}
        }
    }
    // Crude locale check: only POSIX/C/ASCII-only locales force ASCII.
    for var in ["LC_ALL", "LC_CTYPE", "LANG"] {
        if let Ok(v) = std::env::var(var) {
            let upper = v.to_ascii_uppercase();
            if upper == "C" || upper == "POSIX" {
                return false;
            }
            if upper.contains("UTF-8") || upper.contains("UTF8") {
                return true;
            }
        }
    }
    // Default to Unicode — modern terminals all handle it.
    true
}

macro_rules! glyph {
    ($name:ident, $uni:literal, $ascii:literal) => {
        #[doc = concat!("Glyph: `", $uni, "` (Unicode) / `", $ascii, "` (ASCII).")]
        pub fn $name() -> &'static str {
            if use_unicode() {
                $uni
            } else {
                $ascii
            }
        }
    };
}

glyph!(ok, "✓", "[ok]");
glyph!(warn, "⚠", "[!]");
glyph!(info, "ℹ", "[i]");
glyph!(arrow, "▸", ">");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aleph_ascii_env_forces_ascii() {
        std::env::set_var("ALEPH_ASCII", "1");
        assert!(!detect_unicode());
        std::env::remove_var("ALEPH_ASCII");
    }

    #[test]
    fn aleph_ascii_disabled_returns_unicode() {
        std::env::set_var("ALEPH_ASCII", "0");
        assert!(detect_unicode());
        std::env::remove_var("ALEPH_ASCII");
    }

    #[test]
    fn posix_locale_forces_ascii() {
        // Each Rust test shares process env; isolate by setting then clearing
        // every var the function consults.
        let saved: Vec<_> = ["LC_ALL", "LC_CTYPE", "LANG", "ALEPH_ASCII"]
            .iter()
            .map(|k| (*k, std::env::var(k).ok()))
            .collect();
        for (k, _) in &saved {
            std::env::remove_var(k);
        }
        std::env::set_var("LC_ALL", "C");
        assert!(!detect_unicode());
        // restore
        std::env::remove_var("LC_ALL");
        for (k, v) in saved {
            if let Some(v) = v {
                std::env::set_var(k, v);
            }
        }
    }

    #[test]
    fn utf8_locale_returns_unicode() {
        let saved: Vec<_> = ["LC_ALL", "LC_CTYPE", "LANG", "ALEPH_ASCII"]
            .iter()
            .map(|k| (*k, std::env::var(k).ok()))
            .collect();
        for (k, _) in &saved {
            std::env::remove_var(k);
        }
        std::env::set_var("LANG", "en_US.UTF-8");
        assert!(detect_unicode());
        std::env::remove_var("LANG");
        for (k, v) in saved {
            if let Some(v) = v {
                std::env::set_var(k, v);
            }
        }
    }
}
