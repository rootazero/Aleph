//! Automation capability (scripting and Shortcuts).

use async_trait::async_trait;

use crate::automation_types::{ScriptLanguage, ShortcutInfo};
use crate::Result;

/// Execute automation scripts and system shortcuts.
#[async_trait]
pub trait AutomationCapability: Send + Sync {
    /// Run a script in the specified language, returning stdout as a string.
    ///
    /// The call is bounded by a hard timeout (see
    /// [`script_exec::RUN_SCRIPT_TIMEOUT`](crate::script_exec::RUN_SCRIPT_TIMEOUT)):
    /// a script that does not terminate is killed and an error is returned
    /// rather than blocking indefinitely. Long-running processes (dev servers,
    /// watchers) must use [`run_background`](Self::run_background) instead.
    async fn run_script(&self, language: ScriptLanguage, source: &str) -> Result<String>;

    /// Launch a long-running process detached, redirecting its stdout/stderr to
    /// `log_path`, and return the child PID immediately without waiting for it
    /// to exit. This is the correct path for dev servers, file watchers, and
    /// other daemons — unlike [`run_script`](Self::run_script), it does not
    /// block on process completion.
    async fn run_background(
        &self,
        language: ScriptLanguage,
        source: &str,
        log_path: &str,
    ) -> Result<u32>;

    /// List available Shortcuts / automation workflows.
    async fn list_shortcuts(&self) -> Result<Vec<ShortcutInfo>>;

    /// Run a Shortcut by name, with optional input text, returning output as a string.
    async fn run_shortcut(&self, name: &str, input: Option<&str>) -> Result<String>;
}
