//! AppContext - Current application context

use serde::{Deserialize, Serialize};

/// Current application context
///
/// Provides information about the application the user is currently using,
/// which can influence intent routing decisions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppContext {
    /// Application bundle ID (e.g., "com.apple.Notes")
    pub bundle_id: String,

    /// Application name (e.g., "Notes")
    pub app_name: String,

    /// Window title (if available)
    pub window_title: Option<String>,
}

impl AppContext {
    /// Create a new app context
    pub fn new(bundle_id: impl Into<String>, app_name: impl Into<String>) -> Self {
        Self {
            bundle_id: bundle_id.into(),
            app_name: app_name.into(),
            window_title: None,
        }
    }

    /// Create an unknown app context
    pub fn unknown() -> Self {
        Self::new("unknown", "Unknown")
    }

    /// Set window title
    pub fn with_window_title(mut self, title: impl Into<String>) -> Self {
        self.window_title = Some(title.into());
        self
    }

    /// Check if the app matches a bundle ID pattern
    ///
    /// Supports wildcards: `com.apple.*` matches `com.apple.Notes`
    pub fn matches_bundle(&self, pattern: &str) -> bool {
        if let Some(prefix) = pattern.strip_suffix(".*") {
            self.bundle_id.starts_with(prefix)
        } else {
            self.bundle_id == pattern
        }
    }

    /// Check if this is a code editor
    pub fn is_code_editor(&self) -> bool {
        const CODE_EDITORS: &[&str] = &[
            "com.microsoft.VSCode",
            "com.apple.dt.Xcode",
            "com.sublimetext",
            "com.jetbrains",
            "dev.zed.Zed",
            "com.github.atom",
            "io.cursor",
        ];

        CODE_EDITORS
            .iter()
            .any(|editor| self.matches_bundle(editor))
    }

    /// Check if this is a browser
    pub fn is_browser(&self) -> bool {
        const BROWSERS: &[&str] = &[
            "com.apple.Safari",
            "com.google.Chrome",
            "org.mozilla.firefox",
            "com.brave.Browser",
            "com.microsoft.edgemac",
            "company.thebrowser.Browser",
        ];

        BROWSERS.iter().any(|browser| self.matches_bundle(browser))
    }

    /// Check if this is a terminal
    pub fn is_terminal(&self) -> bool {
        const TERMINALS: &[&str] = &[
            "com.apple.Terminal",
            "com.googlecode.iterm2",
            "io.alacritty",
            "com.github.wez.wezterm",
            "dev.warp.Warp-Stable",
        ];

        TERMINALS.iter().any(|term| self.matches_bundle(term))
    }
}

impl Default for AppContext {
    fn default() -> Self {
        Self::unknown()
    }
}
