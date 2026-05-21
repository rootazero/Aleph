use aleph_desktop::automation_types::{ScriptLanguage, ShortcutInfo};
use aleph_desktop::traits::AutomationCapability;
use aleph_desktop::{DesktopError, Result};
use async_trait::async_trait;

pub struct LinuxAutomation;

impl LinuxAutomation {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LinuxAutomation {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AutomationCapability for LinuxAutomation {
    async fn run_script(&self, language: ScriptLanguage, source: &str) -> Result<String> {
        let source = source.to_string();
        match language {
            ScriptLanguage::Shell => {
                let output = tokio::process::Command::new("bash")
                    .arg("-c")
                    .arg(&source)
                    .output()
                    .await
                    .map_err(|e| {
                        DesktopError::InputFailed(format!("Shell script execution failed: {e}"))
                    })?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(DesktopError::InputFailed(format!(
                        "Shell script error: {}",
                        stderr.trim()
                    )));
                }

                Ok(String::from_utf8_lossy(&output.stdout).to_string())
            }
            ScriptLanguage::PowerShell => {
                let result = tokio::process::Command::new("pwsh")
                    .args(["-NoProfile", "-Command", &source])
                    .output()
                    .await;

                let output = match result {
                    Ok(o) => o,
                    Err(_) => tokio::process::Command::new("powershell")
                        .args(["-NoProfile", "-Command", &source])
                        .output()
                        .await
                        .map_err(|e| {
                            DesktopError::InputFailed(format!(
                                "PowerShell execution failed (install pwsh or powershell): {e}"
                            ))
                        })?,
                };

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(DesktopError::InputFailed(format!(
                        "PowerShell error: {}",
                        stderr.trim()
                    )));
                }

                Ok(String::from_utf8_lossy(&output.stdout).to_string())
            }
            ScriptLanguage::AppleScript | ScriptLanguage::Jxa => Err(DesktopError::NotImplemented(
                format!("{language:?} is not available on Linux"),
            )),
        }
    }

    async fn list_shortcuts(&self) -> Result<Vec<ShortcutInfo>> {
        Ok(Vec::new())
    }

    async fn run_shortcut(&self, _name: &str, _input: Option<&str>) -> Result<String> {
        Err(DesktopError::NotImplemented(
            "Shortcuts are not available on Linux".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_default() {
        let _auto = LinuxAutomation::default();
    }

    #[tokio::test]
    async fn test_shell_echo() {
        let auto = LinuxAutomation::new();
        let out = auto
            .run_script(ScriptLanguage::Shell, "echo hello")
            .await
            .unwrap();
        assert!(out.contains("hello"));
    }

    #[tokio::test]
    async fn test_applescript_not_implemented() {
        let auto = LinuxAutomation::new();
        let result = auto
            .run_script(
                ScriptLanguage::AppleScript,
                "tell app \"Finder\" to activate",
            )
            .await;
        assert!(matches!(result, Err(DesktopError::NotImplemented(_))));
    }
}
