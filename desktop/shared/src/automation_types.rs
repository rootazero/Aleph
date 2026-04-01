//! Shared types for automation capabilities (scripting and shortcuts).

use serde::{Deserialize, Serialize};

/// The scripting language for an automation script.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ScriptLanguage {
    /// AppleScript (macOS).
    AppleScript,
    /// JXA — JavaScript for Automation (macOS).
    Jxa,
    /// PowerShell (Windows).
    PowerShell,
    /// Shell script (POSIX sh / bash).
    Shell,
}

/// Information about a Shortcuts / automation workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortcutInfo {
    /// Shortcut name.
    pub name: String,
    /// Identifier, if the platform provides one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Human-readable description of what the shortcut does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}
