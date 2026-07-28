use aleph_desktop::automation_types::{ScriptLanguage, ShortcutInfo};
use aleph_desktop::script_exec::{
    is_spawn_failure, output_capped, spawn_background, RUN_SCRIPT_TIMEOUT,
};
use aleph_desktop::traits::AutomationCapability;
use aleph_desktop::{DesktopError, Result};
use async_trait::async_trait;

pub struct LinuxAutomation;

impl LinuxAutomation {
    #[must_use]
    pub const fn new() -> Self {
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
                let mut cmd = tokio::process::Command::new("bash");
                cmd.arg("-c").arg(&source);
                let output = output_capped(cmd, RUN_SCRIPT_TIMEOUT).await?;

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
                // Run pwsh (cross-platform PowerShell), falling back to the
                // legacy `powershell` binary only when pwsh cannot be launched.
                // Every candidate goes through `output_capped` so a
                // non-terminating script inherits RUN_SCRIPT_TIMEOUT + the
                // kill_on_drop reaper — the bare `.output()` this replaced had
                // neither, so a hung script blocked the agent turn until the
                // harness limit and leaked the child (Shell/macOS/Windows all
                // cap uniformly; Linux PowerShell was the lone gap).
                let mut output = None;
                let mut last_err = None;
                for bin in ["pwsh", "powershell"] {
                    let mut cmd = tokio::process::Command::new(bin);
                    cmd.args(["-NoProfile", "-Command", &source]);
                    match output_capped(cmd, RUN_SCRIPT_TIMEOUT).await {
                        Ok(o) => {
                            output = Some(o);
                            break;
                        }
                        // Fall through to the next interpreter only when THIS
                        // one could not be spawned; a timeout or genuine failure
                        // of an interpreter that *does* exist must not silently
                        // re-run the script under another one.
                        Err(e) if is_spawn_failure(&e) => last_err = Some(e),
                        Err(e) => return Err(e),
                    }
                }
                let output = output.ok_or_else(|| {
                    last_err.unwrap_or_else(|| {
                        DesktopError::InputFailed(
                            "PowerShell execution failed (install pwsh or powershell)".into(),
                        )
                    })
                })?;

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

    async fn run_background(
        &self,
        language: ScriptLanguage,
        source: &str,
        log_path: &str,
    ) -> Result<u32> {
        let cmd = match language {
            ScriptLanguage::Shell => {
                let mut c = tokio::process::Command::new("bash");
                c.arg("-c").arg(source);
                c
            }
            ScriptLanguage::PowerShell => {
                // Try pwsh first, fall back to powershell (older Ubuntu LTS).
                for bin in ["pwsh", "powershell"] {
                    let mut c = tokio::process::Command::new(bin);
                    c.args(["-NoProfile", "-Command", source]);
                    match spawn_background(c, log_path).await {
                        Ok(pid) => return Ok(pid),
                        Err(_) => continue,
                    }
                }
                return Err(DesktopError::InputFailed(
                    "PowerShell not found (install pwsh or powershell)".into(),
                ));
            }
            ScriptLanguage::AppleScript | ScriptLanguage::Jxa => {
                return Err(DesktopError::NotImplemented(format!(
                    "{language:?} is not available on Linux"
                )));
            }
        };
        spawn_background(cmd, log_path).await
    }

    /// Linux has no Shortcuts equivalent, and saying so is not the same as
    /// answering "you have none".
    ///
    /// An empty list reads as *"the feature works, this machine happens to have
    /// no shortcuts"* — which invites the model to suggest creating one, or to
    /// conclude the user deleted theirs. `NotImplemented` says the thing that is
    /// actually true, and matches what [`Self::run_shortcut`] already said; the
    /// two used to disagree about whether the capability exists at all.
    async fn list_shortcuts(&self) -> Result<Vec<ShortcutInfo>> {
        Err(DesktopError::NotImplemented(
            "Shortcuts are a macOS feature with no Linux equivalent. For scripted automation on \
             Linux use automation run_script (shell), or a Skill."
                .into(),
        ))
    }

    async fn run_shortcut(&self, _name: &str, _input: Option<&str>) -> Result<String> {
        Err(DesktopError::NotImplemented(
            "Shortcuts are a macOS feature with no Linux equivalent. For scripted automation on \
             Linux use automation run_script (shell), or a Skill."
                .into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_default() {
        let _ = LinuxAutomation;
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
    async fn shortcuts_report_absence_rather_than_emptiness() {
        // An empty list would read as "the feature works, you just have none",
        // which is a different (and false) claim.
        let auto = LinuxAutomation::new();
        assert!(matches!(
            auto.list_shortcuts().await,
            Err(DesktopError::NotImplemented(_))
        ));
        assert!(matches!(
            auto.run_shortcut("Anything", None).await,
            Err(DesktopError::NotImplemented(_))
        ));
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
