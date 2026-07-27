//! Windows `AutomationCapability` implementation using PowerShell and cmd.

use async_trait::async_trait;
use tokio::process::Command;

use aleph_desktop::automation_types::{ScriptLanguage, ShortcutInfo};
use aleph_desktop::script_exec::{output_capped, spawn_background, RUN_SCRIPT_TIMEOUT};
use aleph_desktop::traits::AutomationCapability;
use aleph_desktop::{DesktopError, Result};

/// Escape a value for safe interpolation into a PowerShell single-quoted
/// string. Doubles embedded single quotes per PowerShell escaping rules,
/// and escapes wildcard characters used by `-Filter`.
fn ps_escape_sq(s: &str) -> String {
    s.replace('\'', "''")
        .replace('[', "`[")
        .replace(']', "`]")
        .replace('*', "`*")
        .replace('?', "`?")
}

/// Build the `powershell.exe`/`cmd.exe` command for a script, without running
/// it. Shared by the synchronous and background execution paths.
fn build_script_cmd(language: ScriptLanguage, source: &str) -> Result<Command> {
    let cmd = match language {
        ScriptLanguage::PowerShell => {
            let mut c = Command::new("powershell.exe");
            c.args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                source,
            ]);
            c
        }
        ScriptLanguage::Shell => {
            let mut c = Command::new("cmd.exe");
            c.args(["/C", source]);
            c
        }
        ScriptLanguage::AppleScript | ScriptLanguage::Jxa => {
            return Err(DesktopError::NotImplemented(
                "AppleScript/JXA not available on Windows".into(),
            ));
        }
    };
    Ok(cmd)
}

pub struct WindowsAutomation {
    _private: (),
}

impl WindowsAutomation {
    #[must_use]
    pub const fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for WindowsAutomation {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AutomationCapability for WindowsAutomation {
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
        #[cfg(windows)]
        {
            let output = Command::new("powershell.exe")
                .args([
                    "-NoProfile",
                    "-Command",
                    r#"Get-ChildItem "$env:APPDATA\Microsoft\Windows\Start Menu\Programs" -Recurse -Filter *.lnk | Select-Object -ExpandProperty BaseName"#,
                ])
                .output()
                .await
                .map_err(|e| DesktopError::InputFailed(format!("failed to list shortcuts: {e}")))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                return Err(DesktopError::InputFailed(format!(
                    "list_shortcuts failed: {stderr}"
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
        #[cfg(not(windows))]
        {
            Err(DesktopError::NotImplemented(
                "list_shortcuts requires Windows".into(),
            ))
        }
    }

    async fn run_shortcut(&self, name: &str, input: Option<&str>) -> Result<String> {
        #[cfg(windows)]
        {
            let name = name.to_string();
            let input = input.map(str::to_string);

            let escaped_name = ps_escape_sq(&name);
            let script = format!(
                r#"$lnk = Get-ChildItem "$env:APPDATA\Microsoft\Windows\Start Menu\Programs" -Recurse -Filter '{0}.lnk' | Select-Object -First 1; if ($lnk) {{ $shell = New-Object -ComObject WScript.Shell; $shortcut = $shell.CreateShortcut($lnk.FullName); & $shortcut.TargetPath $shortcut.Arguments }} else {{ exit 1 }}"#,
                escaped_name
            );

            let mut cmd = Command::new("powershell.exe");
            cmd.args(["-NoProfile", "-Command", &script]);

            if let Some(data) = input {
                cmd.arg(&data);
            }

            let output = cmd.output().await.map_err(|e| {
                DesktopError::InputFailed(format!("failed to run shortcut `{name}`: {e}"))
            })?;

            if output.status.success() {
                Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                Err(DesktopError::InputFailed(format!(
                    "shortcut `{name}` failed: {stderr}"
                )))
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (name, input);
            Err(DesktopError::NotImplemented(
                "run_shortcut requires Windows".into(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_default() {
        let _auto = WindowsAutomation::default();
    }

    #[tokio::test]
    async fn applescript_not_implemented() {
        let auto = WindowsAutomation::new();
        let result = auto
            .run_script(ScriptLanguage::AppleScript, "return 1")
            .await;
        assert!(matches!(result, Err(DesktopError::NotImplemented(_))));
    }
}
