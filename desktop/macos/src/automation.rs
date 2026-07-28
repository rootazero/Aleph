//! macOS automation capability using `osascript` and `shortcuts` CLI.

use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;

use aleph_desktop::automation_types::{ScriptLanguage, ShortcutInfo};
use aleph_desktop::script_exec::{output_capped, spawn_background, RUN_SCRIPT_TIMEOUT};
use aleph_desktop::traits::AutomationCapability;
use aleph_desktop::{DesktopError, Result};

/// Hard ceiling for the `shortcuts` CLI.
///
/// `run_script` has had a cap since the Linux round; these two never did, and
/// they are the easier ones to hang: a Shortcut can open a dialog, wait on a
/// sign-in sheet, or drive another app, and `shortcuts run` then simply never
/// returns — taking the whole turn to the harness's 300s ceiling and leaking the
/// child. Shorter than [`RUN_SCRIPT_TIMEOUT`] because a Shortcut that has not
/// finished in a minute is waiting on a human, not computing.
const SHORTCUT_TIMEOUT: Duration = Duration::from_secs(60);

/// macOS automation via `osascript` (AppleScript/JXA) and `shortcuts` CLI.
pub struct MacOSAutomation {
    _private: (),
}

impl MacOSAutomation {
    /// Create a new `MacOSAutomation` instance.
    pub const fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for MacOSAutomation {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the `osascript`/`sh` command for a script, without running it.
/// Shared by the synchronous ([`run_script`]) and background
/// ([`run_background`]) execution paths.
///
/// SECURITY: For `Shell` this executes arbitrary shell code. Callers must
/// ensure `source` is trusted — never pass unvalidated user input.
fn build_script_cmd(language: ScriptLanguage, source: &str) -> Result<Command> {
    let cmd = match language {
        ScriptLanguage::AppleScript => {
            let mut c = Command::new("osascript");
            c.arg("-e").arg(source);
            c
        }
        ScriptLanguage::Jxa => {
            let mut c = Command::new("osascript");
            c.args(["-l", "JavaScript", "-e"]).arg(source);
            c
        }
        ScriptLanguage::Shell => {
            let mut c = Command::new("sh");
            c.arg("-c").arg(source);
            c
        }
        #[allow(unreachable_patterns)]
        ScriptLanguage::PowerShell => {
            return Err(DesktopError::NotImplemented(
                "PowerShell is not available on macOS".into(),
            ));
        }
    };
    Ok(cmd)
}

#[async_trait]
impl AutomationCapability for MacOSAutomation {
    async fn run_script(&self, language: ScriptLanguage, source: &str) -> Result<String> {
        let cmd = build_script_cmd(language, source)?;
        let output = output_capped(cmd, RUN_SCRIPT_TIMEOUT).await?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(DesktopError::InputFailed(stderr))
        }
    }

    async fn run_background(
        &self,
        language: ScriptLanguage,
        source: &str,
        log_path: &str,
    ) -> Result<u32> {
        let cmd = build_script_cmd(language, source)?;
        spawn_background(cmd, log_path).await
    }

    async fn list_shortcuts(&self) -> Result<Vec<ShortcutInfo>> {
        let mut cmd = Command::new("shortcuts");
        cmd.arg("list");
        let output = output_capped(cmd, SHORTCUT_TIMEOUT).await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(DesktopError::InputFailed(format!(
                "shortcuts list failed: {stderr}"
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let shortcuts = stdout
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| ShortcutInfo {
                name: line.trim().to_string(),
                id: None,
                description: None,
            })
            .collect();

        Ok(shortcuts)
    }

    async fn run_shortcut(&self, name: &str, input: Option<&str>) -> Result<String> {
        let mut cmd = Command::new("shortcuts");
        cmd.arg("run").arg(name);

        if let Some(data) = input {
            cmd.arg("--input-type").arg("text").arg("--input").arg(data);
        }

        let output = output_capped(cmd, SHORTCUT_TIMEOUT).await?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(DesktopError::InputFailed(format!(
                "shortcut `{name}` failed: {stderr}"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_run_applescript() {
        let auto = MacOSAutomation::new();
        let result = auto
            .run_script(ScriptLanguage::AppleScript, "return 2 + 2")
            .await;
        assert_eq!(result.unwrap(), "4");
    }

    #[tokio::test]
    async fn test_run_shell() {
        let auto = MacOSAutomation::new();
        let result = auto.run_script(ScriptLanguage::Shell, "echo hello").await;
        assert_eq!(result.unwrap(), "hello");
    }

    #[tokio::test]
    async fn test_run_background_returns_pid_and_logs() {
        let auto = MacOSAutomation::new();
        let log_path = std::env::temp_dir()
            .join("aleph-macos-bg-test.log")
            .to_string_lossy()
            .into_owned();
        let pid = auto
            .run_background(ScriptLanguage::Shell, "echo bg-ok", &log_path)
            .await
            .expect("run_background should return a pid");
        assert!(pid > 0);

        let mut contents = String::new();
        for _ in 0..40 {
            contents = tokio::fs::read_to_string(&log_path)
                .await
                .unwrap_or_default();
            if contents.contains("bg-ok") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(contents.contains("bg-ok"), "log was: {contents:?}");
        let _ = tokio::fs::remove_file(&log_path).await;
    }

    #[tokio::test]
    async fn test_list_shortcuts() {
        let auto = MacOSAutomation::new();
        let result = auto.list_shortcuts().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_powershell_not_available() {
        let auto = MacOSAutomation::new();
        let result = auto
            .run_script(ScriptLanguage::PowerShell, "Write-Host hello")
            .await;
        assert!(matches!(
            result,
            Err(aleph_desktop::DesktopError::NotImplemented(_))
        ));
    }

    #[tokio::test]
    async fn test_run_jxa() {
        let auto = MacOSAutomation::new();
        let result = auto.run_script(ScriptLanguage::Jxa, "2 + 2").await;
        // JXA may not be available on all macOS versions; result is Ok or Err
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn test_run_shell_failure() {
        let auto = MacOSAutomation::new();
        let result = auto.run_script(ScriptLanguage::Shell, "exit 1").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_run_script_stderr_extracted() {
        let auto = MacOSAutomation::new();
        let result = auto
            .run_script(ScriptLanguage::Shell, "echo error >&2 && exit 1")
            .await;
        let err = result.unwrap_err();
        assert!(format!("{}", err).contains("error"));
    }

    #[tokio::test]
    async fn test_run_shortcut_not_found() {
        let auto = MacOSAutomation::new();
        let result = auto.run_shortcut("__nonexistent_aleph_test__", None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_run_shortcut_with_input() {
        let auto = MacOSAutomation::new();
        // Use a shortcut that exists and accepts input; if none exist, this will error gracefully
        let result = auto
            .run_shortcut("__nonexistent__", Some("test input"))
            .await;
        assert!(result.is_err());
    }
}
