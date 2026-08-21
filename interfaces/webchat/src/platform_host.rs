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
