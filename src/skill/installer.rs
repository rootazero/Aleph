//! Installer — converts `InstallSpec` values into executable shell commands
//! and filters specs by the current operating system.

use crate::domain::skill::{InstallKind, InstallSpec};
use crate::skill::config::InstallPreferences;
use crate::skill::eligibility::current_os;
use crate::utils::no_window::NoWindow;
use serde::Serialize;

/// Build a shell command string for the given install spec.
///
/// Returns `None` for `Download` specs that have no URL.
#[must_use]
pub fn build_install_command(spec: &InstallSpec) -> Option<String> {
    // Validate shell argument: allowlist of safe characters to prevent command injection.
    // Permits alphanumeric, `-`, `_`, `.`, `/`, `@`, `:`, `+`, `=`, `~` (common in
    // package names, Go import paths, and filesystem paths).
    //
    // For `Download` kind the package is interpolated unquoted as `-o <path>` in
    // the curl command, so it must additionally reject path-traversal: a `..`
    // segment would let a malicious manifest write the download outside the
    // intended install directory.
    fn is_safe_shell_arg(s: &str) -> bool {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_./:@+=~".contains(c))
    }

    fn is_safe_path_arg(s: &str) -> bool {
        if !is_safe_shell_arg(s) {
            return false;
        }
        // Reject any `..` segment — naive path-segment scanning is enough for
        // an output-path allowlist; this isn't a full canonical-path check.
        s.split(['/', '\\']).all(|seg| seg != "..")
    }

    if !is_safe_shell_arg(&spec.package) {
        return None;
    }

    match spec.kind {
        InstallKind::Brew => Some(format!("brew install {}", spec.package)),
        InstallKind::Apt => Some(format!("sudo apt-get install -y {}", spec.package)),
        // Scoop — user-space, no UAC; the de-facto CLI/dev-tool manager on
        // Windows. Native to PowerShell, runs unattended by default.
        InstallKind::Scoop => Some(format!("scoop install {}", spec.package)),
        // `-e` = exact id match; the accept flags suppress winget's interactive
        // source/package agreement prompts so the install can run unattended.
        InstallKind::Winget => Some(format!(
            "winget install -e --accept-source-agreements --accept-package-agreements {}",
            spec.package
        )),
        InstallKind::Npm => Some(format!("npm install -g {}", spec.package)),
        InstallKind::Uv => Some(format!("uv pip install {}", spec.package)),
        InstallKind::Go => Some(format!("go install {}", spec.package)),
        InstallKind::Download => spec.url.as_ref().and_then(|url| {
            // Reject URLs containing single quotes (would break shell quoting) or
            // control characters (could confuse curl or the shell).
            if url.contains('\'') || url.chars().any(|c| c.is_control()) {
                return None;
            }
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return None;
            }
            // The output path (`-o`) is interpolated unquoted into the curl
            // command, so it must also pass path-traversal safety — the same
            // allowlist above permits `.` and `/`, which would let a manifest
            // set package: "../../tmp/payload" and write outside the install
            // directory.
            if !is_safe_path_arg(&spec.package) {
                return None;
            }
            Some(format!("curl -fsSL -o {} '{}'", spec.package, url))
        }),
    }
}

/// Filter install specs to only those matching the current OS.
///
/// Specs with no OS restriction (os is `None`) are always included.
#[must_use]
pub fn filter_install_specs_for_current_os(specs: &[InstallSpec]) -> Vec<&InstallSpec> {
    let current = current_os();
    specs
        .iter()
        .filter(|spec| {
            match &spec.os {
                None => true, // No OS restriction — always included
                Some(os_list) => os_list.contains(&current),
            }
        })
        .collect()
}

/// Successful install result — populated only on the `Ok` branch of
/// `install_dependency`. Captures stdout / stderr / exit code of the
/// underlying package-manager invocation so callers (tool output, gateway
/// JSON-RPC responses) can surface them verbatim.
#[derive(Debug, Clone, Serialize)]
pub struct InstallSuccess {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// Errors that can be returned from `install_dependency`. Models the
/// failure modes the install path can produce — missing skill, missing
/// spec, build/exec failure — and surfaces them as a typed
/// `std::error::Error` so callers can use the `?` operator and propagate
/// the cause.
#[derive(Debug, thiserror::Error)]
pub enum SkillInstallError {
    /// The skill id was not found in the registry.
    #[error("Skill not found: {0}")]
    SkillNotFound(String),
    /// No install spec matched the requested `spec_id` or the OS / preference
    /// filter.
    #[error("No matching install spec for skill {0}")]
    NoMatchingSpec(String),
    /// The build / execution failed: the install command could not be
    /// assembled (e.g. the package name failed the shell-arg allowlist) or
    /// the underlying package manager returned a non-zero exit code.
    #[error("Execution failed: {message}")]
    ExecutionFailed {
        message: String,
        stderr: String,
        exit_code: Option<i32>,
    },
    /// I/O error spawning the install command.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Legacy result struct kept for the existing JSON-RPC / builtin-tool wire
/// format. New code should pattern-match on `Result<InstallSuccess,
/// SkillInstallError>` instead; this struct exists only so the
/// `From<InstallResult> for SkillInstallOutput` impl in
/// `builtin_tools/skill_install.rs` and the existing gateway responses
/// can continue to serialise without a breaking change.
#[derive(Debug, Clone, Serialize)]
pub struct InstallResult {
    pub success: bool,
    pub message: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

impl From<Result<InstallSuccess, SkillInstallError>> for InstallResult {
    fn from(r: Result<InstallSuccess, SkillInstallError>) -> Self {
        match r {
            Ok(s) => Self {
                success: true,
                message: "Successfully installed".to_string(),
                stdout: s.stdout,
                stderr: s.stderr,
                exit_code: Some(s.exit_code),
            },
            Err(e) => match e {
                SkillInstallError::SkillNotFound(id) => Self {
                    success: false,
                    message: format!("Skill not found: {id}"),
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: None,
                },
                SkillInstallError::NoMatchingSpec(id) => Self {
                    success: false,
                    message: format!("No matching install spec found for skill {id}"),
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: None,
                },
                SkillInstallError::ExecutionFailed {
                    message,
                    stderr,
                    exit_code,
                } => Self {
                    success: false,
                    message: format!("Failed to install: {message}"),
                    stdout: String::new(),
                    stderr,
                    exit_code,
                },
                SkillInstallError::Io(io) => Self {
                    success: false,
                    message: format!("Failed to execute install command: {io}"),
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: None,
                },
            },
        }
    }
}

const fn install_kind_index(kind: &InstallKind) -> usize {
    match kind {
        InstallKind::Brew => 0,
        InstallKind::Scoop => 1,
        InstallKind::Winget => 2,
        InstallKind::Uv => 3,
        InstallKind::Npm => 4,
        InstallKind::Go => 5,
        InstallKind::Apt => 6,
        InstallKind::Download => 7,
    }
}

// Rank tables for `select_best_install`. Lower values are preferred.
// Scoop and Winget are the Windows system managers, ranked alongside Brew
// (the macOS system manager): high when system managers are preferred, just
// after Brew otherwise. Scoop outranks Winget because it targets CLI/dev
// tools (Winget leans toward desktop apps). OS filtering runs before ranking,
// so these only compete on Windows in practice.
const PREFER_BREW_RANKS: [u8; 8] = [0, 1, 2, 3, 4, 5, 6, 7];
const PREFER_UV_RANKS: [u8; 8] = [2, 3, 4, 0, 1, 5, 6, 7];

const fn install_kind_rank(kind: &InstallKind, prefer_brew: bool) -> u8 {
    let idx = install_kind_index(kind);
    if prefer_brew {
        PREFER_BREW_RANKS[idx]
    } else {
        PREFER_UV_RANKS[idx]
    }
}

/// Select the best install spec for the current platform and preferences.
#[must_use]
pub fn select_best_install<'a>(
    specs: &'a [InstallSpec],
    prefs: &InstallPreferences,
) -> Option<&'a InstallSpec> {
    let mut candidates = filter_install_specs_for_current_os(specs);
    candidates.sort_by_key(|spec| install_kind_rank(&spec.kind, prefs.prefer_brew));
    candidates.into_iter().next()
}

/// Build the shell [`Command`](tokio::process::Command) used to run an install
/// command string.
///
/// On Windows, prefer PowerShell 7 (`pwsh`) when available — it is the native
/// host for `scoop` and modern Windows dev tooling, and (unlike Windows
/// PowerShell 5.1) does not alias `curl` to `Invoke-WebRequest`, so `Download`
/// commands behave as written. Fall back to `cmd /C`, which is always present.
/// `sh` does not exist on Windows by default, so it must never be the Windows
/// shell. On Unix, use `sh -c`.
///
/// Deliberately [`crate::utils::shell::pwsh`] and not
/// [`powershell_host`](crate::utils::shell::powershell_host): the reason above
/// is a reason against 5.1, so this site must skip it rather than inherit the
/// general shell ladder.
fn build_shell_command(cmd_str: &str) -> tokio::process::Command {
    #[cfg(windows)]
    {
        if let Some(pwsh) = crate::utils::shell::pwsh() {
            let mut c = tokio::process::Command::new(&pwsh.program);
            c.arg("-NoProfile").arg("-Command").arg(cmd_str);
            return c;
        }
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/C").arg(cmd_str);
        c
    }
    #[cfg(not(windows))]
    {
        let mut c = tokio::process::Command::new("sh");
        c.arg("-c").arg(cmd_str);
        c
    }
}

/// Executes install commands with timeout and output capture.
pub struct InstallExecutor;

impl InstallExecutor {
    /// Execute an install and return a `Result` so callers can propagate
    /// failures via `?`. The previous signature returned
    /// `InstallResult { success: bool, ... }` which was structurally
    /// indistinguishable from a successful execution by callers that
    /// pattern-matched on `Result<_, _>`.
    pub async fn run(
        spec: &InstallSpec,
        _prefs: &InstallPreferences,
    ) -> Result<InstallSuccess, SkillInstallError> {
        let cmd_str = match build_install_command(spec) {
            Some(cmd) => cmd,
            None => {
                return Err(SkillInstallError::ExecutionFailed {
                    message: format!("Cannot build install command for {}", spec.package),
                    stderr: String::new(),
                    exit_code: None,
                });
            }
        };

        // Pick the platform shell (PowerShell 7 → cmd on Windows, sh on Unix).
        let mut command = build_shell_command(&cmd_str);
        command.kill_on_drop(true);
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(300),
            command.no_window().output(),
        )
        .await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let exit_code = output.status.code();
                if output.status.success() {
                    Ok(InstallSuccess {
                        stdout,
                        stderr,
                        exit_code: exit_code.unwrap_or(0),
                    })
                } else {
                    Err(SkillInstallError::ExecutionFailed {
                        message: format!("Installer exited with status {:?}", output.status),
                        stderr,
                        exit_code,
                    })
                }
            }
            Ok(Err(e)) => Err(SkillInstallError::Io(e)),
            Err(_) => Err(SkillInstallError::ExecutionFailed {
                message: "Installation timed out after 300 seconds".to_string(),
                stderr: String::new(),
                exit_code: None,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::{InstallKind, InstallSpec, Os};
    use crate::skill::config::InstallPreferences;

    fn make_spec(kind: InstallKind, package: &str) -> InstallSpec {
        InstallSpec {
            id: package.to_string(),
            kind,
            package: package.to_string(),
            bins: vec![],
            os: None,
            url: None,
        }
    }

    #[test]
    fn brew_command() {
        let spec = make_spec(InstallKind::Brew, "ripgrep");
        let cmd = build_install_command(&spec).unwrap();
        assert_eq!(cmd, "brew install ripgrep");
    }

    #[test]
    fn apt_command() {
        let spec = make_spec(InstallKind::Apt, "ripgrep");
        let cmd = build_install_command(&spec).unwrap();
        assert_eq!(cmd, "sudo apt-get install -y ripgrep");
    }

    #[test]
    fn scoop_command() {
        let spec = make_spec(InstallKind::Scoop, "ripgrep");
        let cmd = build_install_command(&spec).unwrap();
        assert_eq!(cmd, "scoop install ripgrep");
    }

    #[test]
    fn winget_command() {
        let spec = make_spec(InstallKind::Winget, "BurntSushi.ripgrep.MSVC");
        let cmd = build_install_command(&spec).unwrap();
        assert_eq!(
            cmd,
            "winget install -e --accept-source-agreements --accept-package-agreements BurntSushi.ripgrep.MSVC"
        );
    }

    #[test]
    fn npm_command() {
        let spec = make_spec(InstallKind::Npm, "prettier");
        let cmd = build_install_command(&spec).unwrap();
        assert_eq!(cmd, "npm install -g prettier");
    }

    #[test]
    fn uv_command() {
        let spec = make_spec(InstallKind::Uv, "black");
        let cmd = build_install_command(&spec).unwrap();
        assert_eq!(cmd, "uv pip install black");
    }

    #[test]
    fn go_command() {
        let spec = make_spec(InstallKind::Go, "github.com/golangci/golangci-lint@latest");
        let cmd = build_install_command(&spec).unwrap();
        assert_eq!(cmd, "go install github.com/golangci/golangci-lint@latest");
    }

    #[test]
    fn download_command_with_url() {
        let spec = InstallSpec {
            id: "tool".to_string(),
            kind: InstallKind::Download,
            package: "/usr/local/bin/tool".to_string(),
            bins: vec!["tool".to_string()],
            os: None,
            url: Some("https://example.com/tool".to_string()),
        };
        let cmd = build_install_command(&spec).unwrap();
        assert_eq!(
            cmd,
            "curl -fsSL -o /usr/local/bin/tool 'https://example.com/tool'"
        );
    }

    #[test]
    fn download_command_without_url() {
        let spec = make_spec(InstallKind::Download, "tool");
        let cmd = build_install_command(&spec);
        assert!(cmd.is_none());
    }

    #[test]
    fn os_filter_excludes_wrong_platform() {
        let current = current_os();

        // Spec matching current OS
        let matching = InstallSpec {
            id: "matching".to_string(),
            kind: InstallKind::Brew,
            package: "matching-pkg".to_string(),
            bins: vec![],
            os: Some(vec![current.clone()]),
            url: None,
        };

        // Spec for a different OS
        let wrong_os = match current {
            Os::Darwin => Os::Windows,
            Os::Linux => Os::Windows,
            Os::Windows => Os::Darwin,
        };
        let non_matching = InstallSpec {
            id: "non-matching".to_string(),
            kind: InstallKind::Apt,
            package: "non-matching-pkg".to_string(),
            bins: vec![],
            os: Some(vec![wrong_os]),
            url: None,
        };

        // Spec with no OS restriction (always included)
        let universal = InstallSpec {
            id: "universal".to_string(),
            kind: InstallKind::Npm,
            package: "universal-pkg".to_string(),
            bins: vec![],
            os: None,
            url: None,
        };

        let specs = vec![matching, non_matching, universal];
        let filtered = filter_install_specs_for_current_os(&specs);

        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].id, "matching");
        assert_eq!(filtered[1].id, "universal");
    }

    #[test]
    fn select_best_install_prefers_brew() {
        let specs = vec![
            InstallSpec {
                id: "npm-pkg".into(),
                kind: InstallKind::Npm,
                package: "pkg".into(),
                bins: vec!["pkg".into()],
                os: None,
                url: None,
            },
            InstallSpec {
                id: "brew-pkg".into(),
                kind: InstallKind::Brew,
                package: "pkg".into(),
                bins: vec!["pkg".into()],
                os: None,
                url: None,
            },
        ];

        let prefs = InstallPreferences { prefer_brew: true };

        let best = select_best_install(&specs, &prefs);
        assert!(best.is_some());
        assert_eq!(best.unwrap().id, "brew-pkg");
    }

    #[test]
    fn select_best_install_no_brew_preference() {
        let specs = vec![
            InstallSpec {
                id: "brew-pkg".into(),
                kind: InstallKind::Brew,
                package: "pkg".into(),
                bins: vec!["pkg".into()],
                os: None,
                url: None,
            },
            InstallSpec {
                id: "uv-pkg".into(),
                kind: InstallKind::Uv,
                package: "pkg".into(),
                bins: vec!["pkg".into()],
                os: None,
                url: None,
            },
        ];

        let prefs = InstallPreferences { prefer_brew: false };

        let best = select_best_install(&specs, &prefs);
        assert!(best.is_some());
        assert_eq!(best.unwrap().id, "uv-pkg");
    }

    #[test]
    fn select_best_install_empty() {
        let specs: Vec<InstallSpec> = vec![];
        let prefs = InstallPreferences::default();
        assert!(select_best_install(&specs, &prefs).is_none());
    }

    #[test]
    fn rejects_shell_injection_in_package_name() {
        let dangerous_names = [
            "pkg; rm -rf /",
            "pkg | cat /etc/passwd",
            "pkg & echo pwned",
            "pkg`whoami`",
            "pkg$(id)",
            "pkg > /tmp/out",
            "pkg < /etc/passwd",
            "pkg'injection'",
            "pkg\"injection\"",
            "pkg with spaces",
            "",
        ];
        for name in &dangerous_names {
            let spec = make_spec(InstallKind::Brew, name);
            assert!(
                build_install_command(&spec).is_none(),
                "should reject dangerous package name: {:?}",
                name
            );
        }
    }

    #[test]
    fn accepts_legitimate_package_names() {
        let good_names = [
            "ripgrep",
            "node@18",
            "github.com/user/repo@latest",
            "/usr/local/bin/tool",
            "my-package_v2.0",
        ];
        for name in &good_names {
            let spec = make_spec(InstallKind::Brew, name);
            assert!(
                build_install_command(&spec).is_some(),
                "should accept legitimate package name: {:?}",
                name
            );
        }
    }

    /// The legacy `InstallResult` shape is preserved via the
    /// `From<Result<InstallSuccess, SkillInstallError>>` conversion so
    /// existing JSON-RPC consumers do not break.
    #[test]
    fn install_result_from_result_ok() {
        let r: InstallResult = Ok(InstallSuccess {
            stdout: "ok".into(),
            stderr: String::new(),
            exit_code: 0,
        })
        .into();
        assert!(r.success);
        assert_eq!(r.stdout, "ok");
        assert_eq!(r.exit_code, Some(0));
    }

    #[test]
    fn install_result_from_result_err_skill_not_found() {
        let r: InstallResult = Err(SkillInstallError::SkillNotFound("missing".into())).into();
        assert!(!r.success);
        assert!(r.message.contains("Skill not found"));
    }

    #[test]
    fn install_result_from_result_err_execution_failed() {
        let r: InstallResult = Err(SkillInstallError::ExecutionFailed {
            message: "boom".into(),
            stderr: "traceback".into(),
            exit_code: Some(1),
        })
        .into();
        assert!(!r.success);
        assert_eq!(r.exit_code, Some(1));
        assert_eq!(r.stderr, "traceback");
    }
}
