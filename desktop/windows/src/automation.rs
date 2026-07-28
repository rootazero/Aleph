//! Windows `AutomationCapability` implementation using PowerShell and cmd.

use async_trait::async_trait;
use tokio::process::Command;

use aleph_desktop::automation_types::{ScriptLanguage, ShortcutInfo};
use aleph_desktop::script_exec::{
    hidden_command, output_capped, spawn_background, RUN_SCRIPT_TIMEOUT,
};
use aleph_desktop::traits::AutomationCapability;
use aleph_desktop::{DesktopError, Result};

/// Where Windows keeps Start-menu shortcuts, under both of the roots that hold
/// them.
///
/// Resolved inside PowerShell (`$env:APPDATA` / `$env:ProgramData`) rather than
/// interpolated, so the script text below stays a constant.
const START_MENU_SUBPATH: &str = r"Microsoft\Windows\Start Menu\Programs";

/// The two Start-menu roots, as a PowerShell array expression.
///
/// Only `$env:APPDATA` — the **per-user** Start menu — used to be scanned. That
/// is the smaller half by far: an installer run for all users (which is the
/// default for Office, the browsers, the JetBrains and Visual Studio families,
/// and anything shipped by an MSI) writes into `$env:ProgramData` instead. So
/// `list_shortcuts` reported a handful of entries on a machine with hundreds,
/// and `run_shortcut` answered "no Start-menu shortcut named X was found" for
/// applications sitting in plain sight on the Start menu.
///
/// Roots that do not exist are dropped here so `Get-ChildItem` is never handed a
/// missing path (which, under `$ErrorActionPreference = 'Stop'`, would abort the
/// whole script rather than skip one root).
const START_MENU_ROOTS: &str = r"$roots = @(
    (Join-Path $env:APPDATA '{{SUBPATH}}'),
    (Join-Path $env:ProgramData '{{SUBPATH}}')
) | Where-Object { $_ -and (Test-Path -LiteralPath $_) }
if (-not $roots) { exit 1 }";

/// Environment variables carrying the caller's values into the shortcut script.
///
/// Passing them as environment rather than script text is the whole point: the
/// previous implementation appended `input` to the `powershell.exe` command line
/// *after* `-Command <script>`, and PowerShell documents everything following a
/// string `-Command` as being appended to that command — so tool input was
/// concatenated into the script and executed. Environment variables have no
/// quoting to get wrong.
const ENV_SHORTCUT_NAME: &str = "ALEPH_SHORTCUT_NAME";
const ENV_SHORTCUT_INPUT: &str = "ALEPH_SHORTCUT_INPUT";

/// Resolve a Start-menu shortcut by exact base name and launch it.
///
/// Entirely constant script text: both the name and the input arrive through the
/// environment, and the name is compared with `-eq` rather than fed to `-Filter`
/// so shortcut names containing `[`, `]`, `*` or `?` need no escaping either.
///
/// The `.lnk` is handed to `Start-Process` rather than opened with
/// `WScript.Shell` and its `TargetPath` invoked by hand. A shortcut is more than
/// a path: it also carries the working directory, the window style and the
/// arguments the publisher baked in, and re-implementing the launch dropped all
/// of them (an application whose shortcut sets `Start in` failed to find its own
/// data files). `Start-Process` also returns as soon as the process starts,
/// where `& $target` waited for a console application to exit — up to the full
/// 120 s script ceiling for something whose whole purpose was to be launched.
const RUN_SHORTCUT_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
{{ROOTS}}
$lnk = Get-ChildItem -LiteralPath $roots -Recurse -Filter *.lnk -ErrorAction SilentlyContinue |
    Where-Object { $_.BaseName -eq $env:ALEPH_SHORTCUT_NAME } | Select-Object -First 1
if (-not $lnk) { exit 1 }
if ($env:ALEPH_SHORTCUT_INPUT) {
    Start-Process -FilePath $lnk.FullName -ArgumentList $env:ALEPH_SHORTCUT_INPUT
} else {
    Start-Process -FilePath $lnk.FullName
}
Write-Output "launched $($lnk.FullName)"
"#;

/// List every Start-menu shortcut's base name, one per line.
///
/// `Sort-Object -Unique` because an application installed for all users *and*
/// pinned per user appears under both roots, and the same name twice reads as
/// two applications.
const LIST_SHORTCUTS_SCRIPT: &str = r#"
{{ROOTS}}
Get-ChildItem -LiteralPath $roots -Recurse -Filter *.lnk -ErrorAction SilentlyContinue |
    Select-Object -ExpandProperty BaseName | Sort-Object -Unique
"#;

/// Substitute the two fixed path fragments into a constant script.
///
/// `START_MENU_ROOTS` and `START_MENU_SUBPATH` are compile-time constants, so
/// this is a spelling convenience, not an interpolation point — no caller value
/// passes through it.
fn shortcut_script(template: &str) -> String {
    template
        .replace("{{ROOTS}}", START_MENU_ROOTS)
        .replace("{{SUBPATH}}", START_MENU_SUBPATH)
}

/// Build the `powershell.exe`/`cmd.exe` command for a script, without running
/// it. Shared by the synchronous and background execution paths.
fn build_script_cmd(language: ScriptLanguage, source: &str) -> Result<Command> {
    let cmd = match language {
        ScriptLanguage::PowerShell => {
            let mut c = hidden_command("powershell.exe");
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
            let mut c = hidden_command("cmd.exe");
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
            let mut cmd = hidden_command("powershell.exe");
            cmd.args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &shortcut_script(LIST_SHORTCUTS_SCRIPT),
            ]);
            // A recursive scan of a Start menu on a slow or network-backed
            // profile can stall indefinitely; the synchronous script cap is the
            // same ceiling `run_script` uses.
            let output = output_capped(cmd, RUN_SCRIPT_TIMEOUT).await?;

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

    /// Run a Start-menu shortcut by name, optionally handing it one extra
    /// argument.
    ///
    /// Both values travel as environment variables, never as script text. The
    /// previous implementation appended `input` to the `powershell.exe` command
    /// line after `-Command <script>`, which PowerShell documents as *appending
    /// to the command* — so anything in `input` was executed as PowerShell.
    async fn run_shortcut(&self, name: &str, input: Option<&str>) -> Result<String> {
        #[cfg(windows)]
        {
            let mut cmd = hidden_command("powershell.exe");
            cmd.args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &shortcut_script(RUN_SHORTCUT_SCRIPT),
            ]);
            cmd.env(ENV_SHORTCUT_NAME, name);
            cmd.env(ENV_SHORTCUT_INPUT, input.unwrap_or_default());

            let output = output_capped(cmd, RUN_SCRIPT_TIMEOUT).await?;

            if output.status.success() {
                Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                let detail = if stderr.is_empty() {
                    format!("no Start-menu shortcut named `{name}` was found")
                } else {
                    stderr
                };
                Err(DesktopError::InputFailed(format!(
                    "shortcut `{name}` failed: {detail}"
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

    #[test]
    fn shortcut_scripts_interpolate_nothing_the_caller_controls() {
        // The regression this pins: shortcut name and input used to reach
        // PowerShell as script text. Both scripts must mention the environment
        // variables and must contain no `{}` format hole at all.
        for template in [RUN_SHORTCUT_SCRIPT, LIST_SHORTCUTS_SCRIPT] {
            let script = shortcut_script(template);
            assert!(
                !script.contains("{{"),
                "no placeholder may survive into the script: {script}"
            );
            assert!(
                script.contains(START_MENU_SUBPATH),
                "the Start-menu path must be present"
            );
        }
        assert!(RUN_SHORTCUT_SCRIPT.contains(ENV_SHORTCUT_NAME));
        assert!(RUN_SHORTCUT_SCRIPT.contains(ENV_SHORTCUT_INPUT));
    }

    #[test]
    fn both_start_menu_roots_are_scanned() {
        // Scanning only `$env:APPDATA` hid every all-users install — which is
        // most of them — from both listing and launching.
        for template in [RUN_SHORTCUT_SCRIPT, LIST_SHORTCUTS_SCRIPT] {
            let script = shortcut_script(template);
            assert!(
                script.contains("$env:APPDATA"),
                "per-user Start menu must be scanned"
            );
            assert!(
                script.contains("$env:ProgramData"),
                "all-users Start menu must be scanned"
            );
            assert!(
                script.contains("-LiteralPath $roots"),
                "both roots must be handed to the same enumeration"
            );
        }
    }

    #[test]
    fn a_shortcut_is_launched_through_the_lnk_not_its_target() {
        // Re-implementing the launch from `TargetPath` dropped the working
        // directory and window style the shortcut carries, and blocked until a
        // console target exited.
        assert!(RUN_SHORTCUT_SCRIPT.contains("Start-Process -FilePath $lnk.FullName"));
        assert!(
            !RUN_SHORTCUT_SCRIPT.contains("WScript.Shell"),
            "the shortcut should not be re-opened via COM"
        );
    }

    #[test]
    fn the_shortcut_name_is_matched_exactly_not_as_a_wildcard() {
        // `-Filter '<name>.lnk'` treated `[`, `]`, `*` and `?` as wildcards,
        // which is why an escape helper existed at all. Exact `-eq` matching
        // removes both the wildcards and the escaping.
        assert!(RUN_SHORTCUT_SCRIPT.contains("$_.BaseName -eq $env:ALEPH_SHORTCUT_NAME"));
    }
}
