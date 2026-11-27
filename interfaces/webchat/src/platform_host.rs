//! Which platform's WebView is rendering this Panel.
//!
//! A **pure reader** of the `data-platform` attribute on `<html>`. The
//! resolution — including the UA fallback for pages the shell's
//! `initialization_script` never reached — belongs to `baseline-probe.js`,
//! which runs synchronously before the WASM boots and writes the attribute
//! (see the spec, section 3.2). Duplicating that fallback here would be a
//! second implementation of the same decision.
//!
//! Sibling of `crate::platform::wide::views::voice::audio::is_native_shell`,
//! which reads `data-shell` the same way and for the same reason: these facts
//! are declared by the host, not derived in WASM, where `cfg(target_os)` is
//! always `unknown`.

/// The three WebView engines Aleph ships against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostPlatform {
    /// WKWebView.
    MacOs,
    /// Edge WebView2 (Chromium).
    Windows,
    /// WebKitGTK.
    Linux,
}

impl HostPlatform {
    /// Parse the `data-platform` value. Anything unrecognised — including a
    /// missing attribute — is `Linux`: that is the direction the probe already
    /// chose for its own ambiguous case, because flat rendering is a
    /// degradation and never a hazard.
    #[must_use]
    pub fn from_attribute(value: Option<&str>) -> Self {
        match value {
            Some("macos") => Self::MacOs,
            Some("windows") => Self::Windows,
            _ => Self::Linux,
        }
    }
}

/// This document's host platform.
#[cfg(target_arch = "wasm32")]
#[must_use]
pub fn host() -> HostPlatform {
    let attr = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
        .and_then(|e| e.get_attribute("data-platform"));
    HostPlatform::from_attribute(attr.as_deref())
}

/// Non-wasm builds (unit tests on the host toolchain) have no document.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn host() -> HostPlatform {
    HostPlatform::Linux
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Strip `/* … */` comments so the scan below reads selectors, not prose.
    /// The rule this file enforces is *about* `data-platform="macos"`, so the
    /// comments explaining it necessarily quote the very string being hunted —
    /// scanning the raw text would make the guard red on its own documentation.
    fn strip_css_comments(css: &str) -> String {
        let mut out = String::with_capacity(css.len());
        let mut rest = css;
        while let Some(open) = rest.find("/*") {
            out.push_str(&rest[..open]);
            match rest[open + 2..].find("*/") {
                Some(close) => rest = &rest[open + 2 + close + 2..],
                None => return out,
            }
        }
        out.push_str(rest);
        out
    }

    /// Scan every stylesheet delivered to a browser. A macOS browser and
    /// WebView share the engine marker, so window rules need a shell marker.
    fn panel_stylesheets() -> Vec<(String, String)> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("styles");
        let mut out: Vec<(String, String)> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                if path.extension()? != "css" {
                    return None;
                }
                let name = path.file_name()?.to_string_lossy().into_owned();
                Some((name, std::fs::read_to_string(&path).ok()?))
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        // Self-check on the derivation itself: a wrong directory, a renamed
        // tree, or a filter that stopped matching yields an empty list, and an
        // empty list satisfies every assertion below without reading a byte.
        assert!(
            out.iter().any(|(name, _)| name == "tailwind.css"),
            "derived no tailwind.css from {} — the stylesheet directory moved \
             and this guard is scanning nothing",
            dir.display()
        );
        out
    }

    #[test]
    fn stylesheet_never_keys_window_chrome_on_the_os_alone() {
        let mut bare: Vec<String> = Vec::new();
        for (name, raw) in panel_stylesheets() {
            let css = strip_css_comments(&raw);
            for (i, line) in css.lines().enumerate() {
                if !line.contains("data-platform=\"macos\"") {
                    continue;
                }
                if !line.contains("[data-shell=\"aleph-nsh\"]") {
                    bare.push(format!("  {name}:{}: {}", i + 1, line.trim()));
                }
            }
        }

        assert!(
            bare.is_empty(),
            "these macOS selectors also apply in a normal browser; scope \
             any native window rule to `[data-shell=\"aleph-nsh\"]`:\n{}",
            bare.join("\n")
        );
    }

    #[test]
    fn known_values_map_to_their_platform() {
        assert_eq!(
            HostPlatform::from_attribute(Some("macos")),
            HostPlatform::MacOs
        );
        assert_eq!(
            HostPlatform::from_attribute(Some("windows")),
            HostPlatform::Windows
        );
        assert_eq!(
            HostPlatform::from_attribute(Some("linux")),
            HostPlatform::Linux
        );
    }

    #[test]
    fn absent_or_unknown_resolves_to_linux_the_safe_direction() {
        // Flat rendering is a degradation, never a hazard, so an unknown host
        // gets the conservative answer — the same choice baseline-probe.js
        // makes for an unrecognised user agent.
        assert_eq!(HostPlatform::from_attribute(None), HostPlatform::Linux);
        assert_eq!(HostPlatform::from_attribute(Some("")), HostPlatform::Linux);
        assert_eq!(
            HostPlatform::from_attribute(Some("haiku")),
            HostPlatform::Linux
        );
    }
}
