//! Which platform's WebView is rendering this Panel.
//!
//! A **pure reader** of the `data-platform` attribute on `<html>`. The
//! resolution — including the UA fallback for pages the shell's
//! `initialization_script` never reached — belongs to `baseline-probe.js`,
//! which runs synchronously before the WASM boots and writes the attribute
//! (see the spec, section 3.2). Duplicating that fallback here would be a
//! second implementation of the same decision.
//!
//! Sibling of [`crate::platform::wide::views::voice::audio::is_native_shell`],
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

    /// The Panel stylesheet may not key **window chrome** on the OS alone.
    ///
    /// `data-platform` answers *which WebView engine is rendering this
    /// document*. `baseline-probe.js` resolves it from the user agent whenever
    /// no host declared one, so it reads `macos` in a plain Chrome or Safari on
    /// a Mac exactly as it does inside the desktop app — that is correct, and
    /// the things that genuinely depend on the engine (flat-mode degradation,
    /// the Linux decoder advice above) are right to read it.
    ///
    /// The chrome-layout rules are a different fact. They exist because the
    /// Tauri window is built `transparent(true)` with vibrancy behind it and
    /// carries `TitleBarStyle::Overlay`, so the traffic lights float over the
    /// top-left of the *content* and every chrome glyph has to step around
    /// them. In a browser there is no such window, and the two facts diverge.
    ///
    /// They diverged silently once: every one of those rules was keyed on
    /// `html[data-platform="macos"]` while its own comment said "macOS Tauri",
    /// so on macOS-in-a-browser the Panel reserved 30 px for a titlebar that
    /// was not there, indented the collapse toggle 72 px to clear traffic
    /// lights that were not there, and hid the inline brand-row button that was
    /// supposed to take over — leaving one button floating in empty space above
    /// the brand row. The discriminator already existed and was already used
    /// correctly by `is_native_shell`; only the stylesheet ignored it.
    ///
    /// So: an occurrence of `data-platform="macos"` in a *selector* must carry
    /// `[data-shell="aleph-tauri"]` alongside it. Anything that truly depends
    /// on the engine and not on the window belongs in a media/support query or
    /// in Rust, not in a bare platform selector.
    /// Every stylesheet the Panel ships, derived from the directory rather
    /// than listed here. The rule below is about what a *browser* renders, so
    /// its scope is "CSS that reaches a browser" — and `styles/` is that set.
    /// Naming one file instead would leave the guard blind to the next one
    /// added beside it: `ios.css` already exists and is `include_str!`d into
    /// the phone shell, and a hardcoded `tailwind.css` never looked at it.
    ///
    /// Shell-injected documents are deliberately NOT in scope and are not an
    /// exemption list: `desktop/shell/splash/*.html` and the update banner in
    /// `desktop/shell/src/update.rs` are markup the shell itself puts on
    /// screen, so `data-platform="macos"` there cannot be reached by a browser
    /// and already means "macOS shell". They live in a different crate's
    /// directory, so the derivation excludes them by construction.
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
        let mut paired = 0usize;
        for (name, raw) in panel_stylesheets() {
            let css = strip_css_comments(&raw);
            for (i, line) in css.lines().enumerate() {
                if !line.contains("data-platform=\"macos\"") {
                    continue;
                }
                if line.contains("[data-shell=\"aleph-tauri\"]") {
                    paired += 1;
                } else {
                    bare.push(format!("  {name}:{}: {}", i + 1, line.trim()));
                }
            }
        }

        // Self-check: a stylesheet that no longer mentions the pair at all
        // would satisfy the assertion below vacuously — a renamed file, a
        // reorganised section, or a scan that silently stopped matching all
        // look identical to "the rule is upheld" without this.
        assert!(
            paired > 0,
            "no `[data-shell=\"aleph-tauri\"][data-platform=\"macos\"]` selector \
             found in any Panel stylesheet — the macOS-shell chrome rules moved \
             or the scan stopped matching them, and this guard is measuring nothing"
        );

        assert!(
            bare.is_empty(),
            "a Panel stylesheet keys window chrome on the OS alone; each of \
             these also applies in a browser on macOS, where there is no \
             overlay titlebar and no transparent window. Pair the selector \
             with `[data-shell=\"aleph-tauri\"]`:\n{}",
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
