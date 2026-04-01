//! Automation capability (scripting and Shortcuts).

use async_trait::async_trait;

use crate::automation_types::*;
use crate::Result;

/// Execute automation scripts and system shortcuts.
#[async_trait]
pub trait AutomationCapability: Send + Sync {
    /// Run a script in the specified language, returning stdout as a string.
    async fn run_script(&self, language: ScriptLanguage, source: &str) -> Result<String>;

    /// List available Shortcuts / automation workflows.
    async fn list_shortcuts(&self) -> Result<Vec<ShortcutInfo>>;

    /// Run a Shortcut by name, with optional input text, returning output as a string.
    async fn run_shortcut(&self, name: &str, input: Option<&str>) -> Result<String>;
}
