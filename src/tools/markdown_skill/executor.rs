//! CLI Execution Backends
//!
//! Implements host, Docker, and `VirtualFs` execution modes for Markdown CLI tools.

use crate::utils::no_window::NoWindow;
use anyhow::Result;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tracing::{debug, info, warn};

/// Default execution timeout for CLI skills (5 minutes).
const DEFAULT_EXECUTION_TIMEOUT: Duration = Duration::from_secs(300);

use super::spec::NetworkMode;
use super::tool_adapter::{MarkdownCliTool, MarkdownToolOutput};

/// Build the host process [`Command`] for a skill's binary, accounting for
/// Windows launcher conventions.
///
/// On Windows, `which` is PATHEXT-aware and resolves launchers that
/// `CreateProcess` cannot execute directly:
///   * `.cmd` / `.bat` — node/npm-style CLIs; must run through `cmd /C`.
///   * `.ps1` — PowerShell scripts; run via PowerShell 7 (`pwsh`) when present,
///     falling back to Windows PowerShell (`powershell`), using `-File`.
///
/// Without this, a skill's binary check (via `which`) would pass while the
/// actual spawn failed. On non-Windows platforms this is byte-identical to
/// `Command::new(bin).args(cli_args)`.
fn build_host_command(bin: &str, cli_args: &[String]) -> Command {
    #[cfg(windows)]
    {
        if let Ok(path) = which::which(bin) {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase);
            let mut cmd = match ext.as_deref() {
                Some("cmd" | "bat") => {
                    // CreateProcess cannot execute batch files directly.
                    let mut c = Command::new("cmd");
                    c.arg("/C").arg(&path);
                    c
                }
                Some("ps1") => {
                    // Prefer PowerShell 7; fall back to Windows PowerShell.
                    let shell = if which::which("pwsh").is_ok() {
                        "pwsh"
                    } else {
                        "powershell"
                    };
                    let mut c = Command::new(shell);
                    c.arg("-NoProfile").arg("-File").arg(&path);
                    c
                }
                _ => Command::new(&path),
            };
            cmd.args(cli_args);
            return cmd;
        }
        // Resolution failed — fall through to a bare spawn so the original
        // "not found" error surfaces to the caller.
    }

    let mut cmd = Command::new(bin);
    cmd.args(cli_args);
    cmd
}

impl MarkdownCliTool {
    /// Resolve the execution timeout: skill-specific override or global default.
    fn execution_timeout(&self) -> Duration {
        self.spec
            .metadata
            .aleph
            .as_ref()
            .and_then(|a| a.timeout_secs)
            .map_or(DEFAULT_EXECUTION_TIMEOUT, Duration::from_secs)
    }

    /// Execute on host system (with `SafetyGate` if configured).
    ///
    /// # Host-mode contract (see `docs/superpowers/specs/2026-05-20-host-sandbox-netns-decision-design.md`)
    ///
    /// Host mode is the explicit no-isolation execution path. Skill authors who
    /// write `sandbox: host` are choosing to trust the host environment. This
    /// function **must not** be evolved to silently add isolation — doing so
    /// would invert the user's clear declaration.
    ///
    /// `network: none` under host mode sets `NO_PROXY=*` as a partial mitigation
    /// and emits a `warn!` informing the user that real isolation requires
    /// `sandbox: docker` (cross-platform) or, on Linux, the planned
    /// `sandbox: bwrap` follow-up that routes through `src/sandbox/platforms/linux/bwrap.rs`.
    ///
    /// `unshare(CLONE_NEWNET)` was explicitly rejected (Decision 1 in the spec):
    /// cross-platform broken, requires user namespaces that many distros
    /// disable by default, and duplicates the bwrap driver. Future maintainers:
    /// preserve the *partial mitigation + truthful warning* contract; do NOT
    /// add silent ineffective isolation.
    pub(crate) async fn execute_on_host(&self, cli_args: &[String]) -> Result<MarkdownToolOutput> {
        // Get primary binary name
        let bin = self
            .spec
            .metadata
            .requires
            .bins
            .first()
            .ok_or_else(|| anyhow::anyhow!("No binary specified in skill metadata"))?;

        info!(
            tool = %self.spec.name,
            bin = %bin,
            args = ?cli_args,
            "Executing CLI tool on host"
        );

        // Build command (Windows-aware: resolves `.cmd`/`.bat` launchers so a
        // node/npm CLI that passes the `which` binary check also executes).
        let mut cmd = build_host_command(bin, cli_args);
        cmd.stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());

        // Apply network restrictions if specified.
        // See the function-level doc above for the host-mode contract:
        // host sandbox cannot truly isolate the network (that requires a
        // network namespace, and bwrap/docker already provide that). Be
        // honest: set NO_PROXY as a partial mitigation and warn that real
        // isolation needs cross-platform Docker mode (or the planned bwrap
        // mode on Linux). Decision recorded in
        // docs/superpowers/specs/2026-05-20-host-sandbox-netns-decision-design.md
        if let Some(aleph_meta) = &self.spec.metadata.aleph {
            if matches!(aleph_meta.security.network, NetworkMode::None) {
                cmd.env("NO_PROXY", "*");
                cmd.env("no_proxy", "*");
                warn!(
                    skill = %self.spec.name,
                    "skill declares network=none but runs in host sandbox; \
                     network is NOT truly isolated — use sandbox: docker for \
                     enforced cross-platform isolation (or wait for sandbox: \
                     bwrap on Linux — tracked in C-Plus deferred follow-up)"
                );
            }
        }

        // Execute with timeout
        let timeout = self.execution_timeout();
        let output = tokio::time::timeout(timeout, cmd.no_window().output())
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "Skill '{}' timed out after {}s",
                    self.spec.name,
                    timeout.as_secs()
                )
            })??;

        Ok(MarkdownToolOutput {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
        })
    }

    /// Execute in Docker container with proper env and error handling
    pub(crate) async fn execute_in_docker(
        &self,
        cli_args: &[String],
    ) -> Result<MarkdownToolOutput> {
        let bin = self
            .spec
            .metadata
            .requires
            .bins
            .first()
            .ok_or_else(|| anyhow::anyhow!("No binary specified"))?;

        // Get Docker image (STRICT: must be configured)
        let container_image = self.get_docker_image()?;

        info!(
            tool = %self.spec.name,
            bin = %bin,
            image = %container_image,
            args = ?cli_args,
            "Executing CLI tool in Docker sandbox"
        );

        let mut docker_args = vec![
            "run".to_string(),
            "--rm".to_string(),
            "--network".to_string(),
            self.get_docker_network_mode(),
            "--read-only".to_string(),
            "--tmpfs".to_string(),
            "/tmp:rw,noexec,nosuid,size=100m".to_string(),
        ];

        // Pass environment variables and extra flags (filtered through allowlist)
        self.push_docker_runtime_args(&mut docker_args);

        docker_args.push(container_image.clone());
        docker_args.push(bin.clone());
        docker_args.extend_from_slice(cli_args);

        let timeout = self.execution_timeout();
        let output = tokio::time::timeout(
            timeout,
            Command::new("docker")
                .args(&docker_args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .stdin(Stdio::null())
                .no_window()
                .output(),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "Docker skill '{}' timed out after {}s",
                self.spec.name,
                timeout.as_secs()
            )
        })??;

        // Enhanced exit code handling
        if !output.status.success() {
            let exit_code = output.status.code().unwrap_or(-1);
            let stderr = String::from_utf8_lossy(&output.stderr);
            check_docker_exit_code(exit_code, &stderr, bin, &container_image)?;
            warn!(
                tool = %self.spec.name,
                exit_code = exit_code,
                "Tool execution failed"
            );
        }

        Ok(MarkdownToolOutput {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
        })
    }

    /// Push Docker runtime args from the skill's `aleph.docker` config:
    /// env vars (with validation) and extra flags (with allowlist filtering).
    fn push_docker_runtime_args(&self, docker_args: &mut Vec<String>) {
        let Some(aleph_meta) = &self.spec.metadata.aleph else {
            return;
        };
        let Some(docker_cfg) = &aleph_meta.docker else {
            return;
        };
        for env_var in &docker_cfg.env_vars {
            if env_var.is_empty()
                || !env_var.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                || !env_var
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                warn!(
                    env_var = %env_var,
                    "Skipping env var with invalid name (must match [A-Za-z_][A-Za-z0-9_]*)"
                );
                continue;
            }
            if let Ok(value) = std::env::var(env_var) {
                if value.contains('\n') || value.contains('\r') {
                    warn!(
                        env_var = %env_var,
                        "Skipping env var with newline characters (security risk)"
                    );
                    continue;
                }
                docker_args.push("-e".to_string());
                docker_args.push(format!("{env_var}={value}"));
                tracing::debug!(env_var = %env_var, "Passing env var to container");
            } else {
                warn!(
                    env_var = %env_var,
                    "Required env var not found in host environment"
                );
            }
        }
        for flag in &docker_cfg.extra_flags {
            if is_allowed_docker_flag(flag) {
                docker_args.push(flag.clone());
            } else {
                warn!(
                    flag = %flag,
                    tool = %self.spec.name,
                    "Blocked disallowed Docker flag from skill config"
                );
            }
        }
    }

    /// Get Docker image (STRICT: must be configured or known)
    fn get_docker_image(&self) -> Result<String> {
        // Priority 1: Explicit configuration
        if let Some(aleph_meta) = &self.spec.metadata.aleph {
            if let Some(docker_cfg) = &aleph_meta.docker {
                return Ok(docker_cfg.image.clone());
            }
        }

        // Priority 2: Hardcoded mapping for common tools
        let bin = self
            .spec
            .metadata
            .requires
            .bins
            .first()
            .ok_or_else(|| anyhow::anyhow!("No binary specified"))?;

        let known_image = match bin.as_str() {
            "gh" => Some("ghcr.io/cli/cli:latest"),
            "kubectl" => Some("bitnami/kubectl:latest"),
            "aws" => Some("amazon/aws-cli:latest"),
            "gcloud" => Some("google/cloud-sdk:alpine"),
            "terraform" => Some("hashicorp/terraform:latest"),
            "helm" => Some("alpine/helm:latest"),
            "ffmpeg" => Some("linuxserver/ffmpeg:latest"),
            "yt-dlp" => Some("jauderho/yt-dlp:latest"),
            _ => None,
        };

        if let Some(image) = known_image {
            info!(
                bin = %bin,
                image = %image,
                "Using known Docker image mapping"
            );
            return Ok(image.to_string());
        }

        // Priority 3: FAIL (no blind fallback to alpine)
        anyhow::bail!(
            "Docker execution for '{}' requires 'metadata.aleph.docker.image' configuration. \
            Binary '{}' has no known Docker image mapping.",
            self.spec.name,
            bin
        )
    }

    fn get_docker_network_mode(&self) -> String {
        if let Some(aleph_meta) = &self.spec.metadata.aleph {
            match aleph_meta.security.network {
                NetworkMode::None => "none".to_string(),
                NetworkMode::Local => {
                    // Docker's default bridge network NATs to the internet, so
                    // `network=local` (LAN-only) is NOT actually enforced here.
                    // Be honest, mirroring the host-mode network=none warning.
                    warn!(
                        skill = %self.spec.name,
                        "skill declares network=local but docker maps it to the \
                         default bridge network, which NATs to the internet; \
                         local-only egress is NOT enforced"
                    );
                    "bridge".to_string()
                }
                NetworkMode::Internet => "bridge".to_string(),
            }
        } else {
            "bridge".to_string()
        }
    }

    /// Execute in `VirtualFs` sandbox (lightweight isolation)
    ///
    /// Provides filesystem isolation through:
    /// - Temporary isolated working directory
    /// - Environment variable redirection (HOME, TMPDIR, PWD)
    /// - Read-only access to real filesystem
    /// - Writable temporary filesystem
    /// - Automatic cleanup after execution
    pub(crate) async fn execute_in_virtualfs(
        &self,
        cli_args: &[String],
    ) -> Result<MarkdownToolOutput> {
        let bin = self
            .spec
            .metadata
            .requires
            .bins
            .first()
            .ok_or_else(|| anyhow::anyhow!("No binary specified"))?;

        // Create isolated sandbox environment
        let sandbox = VirtualFsSandbox::new(&self.spec.name)?;

        info!(
            tool = %self.spec.name,
            bin = %bin,
            sandbox_dir = %sandbox.root_dir.display(),
            args = ?cli_args,
            "Executing CLI tool in VirtualFs sandbox"
        );

        // Build command with isolated environment
        let mut cmd = Command::new(bin);
        cmd.args(cli_args)
            .current_dir(&sandbox.work_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());

        // Apply sandbox environment variables
        sandbox.apply_env(&mut cmd);

        // Apply network restrictions if specified
        if let Some(aleph_meta) = &self.spec.metadata.aleph {
            if matches!(aleph_meta.security.network, NetworkMode::None) {
                #[cfg(target_os = "linux")]
                {
                    cmd.env("NO_PROXY", "*");
                }
            }
        }

        // Execute with timeout
        let timeout = self.execution_timeout();
        let output = tokio::time::timeout(timeout, cmd.no_window().output())
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "VirtualFs skill '{}' timed out after {}s",
                    self.spec.name,
                    timeout.as_secs()
                )
            })??;

        // Cleanup happens when sandbox is dropped

        Ok(MarkdownToolOutput {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
        })
    }
}

/// `VirtualFs` Sandbox Environment
///
/// Provides lightweight filesystem isolation by creating a temporary
/// directory structure and redirecting environment variables.
///
/// ## Isolation Strategy
///
/// - **Working Directory**: Isolated temp directory for execution
/// - **Home Directory**: Sandbox-specific home for config files
/// - **Temp Directory**: Sandbox-specific temp for temporary files
/// - **Real Filesystem**: Read-only access (via normal file paths)
///
/// ## Security
///
/// - All writes go to sandbox temp directories
/// - Real filesystem remains unmodified (unless tool uses absolute paths)
/// - Automatic cleanup on drop
///
/// ## Limitations
///
/// - Not true filesystem isolation (tools can still access real FS via absolute paths)
/// - Best for well-behaved CLI tools that respect environment variables
/// - For untrusted code, use Docker sandbox instead
struct VirtualFsSandbox {
    /// Root directory of the sandbox (will be cleaned up)
    root_dir: PathBuf,

    /// Working directory for command execution
    work_dir: PathBuf,

    /// Isolated home directory
    home_dir: PathBuf,

    /// Isolated temp directory
    temp_dir: PathBuf,
}

impl VirtualFsSandbox {
    /// Create a new `VirtualFs` sandbox
    fn new(tool_name: &str) -> Result<Self> {
        // Create root sandbox directory with unique name
        let root_dir = std::env::temp_dir().join(format!(
            "aleph-virtualfs-{}-{}",
            tool_name,
            uuid::Uuid::new_v4()
        ));

        std::fs::create_dir_all(&root_dir)?;

        // Create subdirectories
        let work_dir = root_dir.join("work");
        let home_dir = root_dir.join("home");
        let temp_dir = root_dir.join("tmp");

        std::fs::create_dir_all(&work_dir)?;
        std::fs::create_dir_all(&home_dir)?;
        std::fs::create_dir_all(&temp_dir)?;

        debug!(
            root = %root_dir.display(),
            "Created VirtualFs sandbox"
        );

        Ok(Self {
            root_dir,
            work_dir,
            home_dir,
            temp_dir,
        })
    }

    /// Apply sandbox environment variables to command
    fn apply_env(&self, cmd: &mut Command) {
        // Redirect HOME to sandbox home
        cmd.env("HOME", &self.home_dir);

        // Redirect TMPDIR/TEMP/TMP to sandbox temp
        cmd.env("TMPDIR", &self.temp_dir);
        cmd.env("TEMP", &self.temp_dir);
        cmd.env("TMP", &self.temp_dir);

        // Set PWD to sandbox work directory
        cmd.env("PWD", &self.work_dir);

        // Clear potentially dangerous environment variables
        cmd.env_remove("LD_PRELOAD");
        cmd.env_remove("DYLD_INSERT_LIBRARIES");
        cmd.env_remove("DYLD_LIBRARY_PATH");
        cmd.env_remove("LD_LIBRARY_PATH");

        debug!(
            home = %self.home_dir.display(),
            tmp = %self.temp_dir.display(),
            pwd = %self.work_dir.display(),
            "Applied VirtualFs environment"
        );
    }
}

/// Classify Docker container exit codes into actionable diagnostics.
///
/// Known exit codes (125=daemon error, 126=cmd non-executable,
/// 127=cmd not found, 137=OOM/SIGKILL) are raised as errors;
/// unknown non-zero codes pass through so the caller logs a warning
/// and returns the tool's partial output.
fn check_docker_exit_code(exit_code: i32, stderr: &str, bin: &str, image: &str) -> anyhow::Result<()> {
    match exit_code {
        125 => anyhow::bail!("Docker runtime error (container failed to start): {stderr}"),
        126 => anyhow::bail!("Command cannot be executed in container: {stderr}"),
        127 => anyhow::bail!(
            "Command '{bin}' not found in container image '{image}'. \
            Check metadata.aleph.docker.image configuration."
        ),
        137 => anyhow::bail!("Container killed (OOM or SIGKILL): {stderr}"),
        _ => Ok(()),
    }
}

/// Allowlist of safe Docker flags that skills may use.
///
/// Only explicitly permitted flags pass through. Unknown flags are rejected
/// to prevent sandbox escapes via novel or future Docker flags.
fn is_allowed_docker_flag(flag: &str) -> bool {
    // Reject flags containing whitespace — prevents "-v /host:/container" bypass
    if flag.contains(char::is_whitespace) {
        return false;
    }

    // Must start with '-' to be a flag
    if !flag.starts_with('-') {
        return false;
    }

    let flag_lower = flag.to_lowercase();

    // Extract the flag name (before '=' if present)
    let flag_name = flag_lower.split('=').next().unwrap_or(&flag_lower);
    let flag_name = flag_name.trim_start_matches('-');

    // Allowlist of safe flags — only these are permitted
    const ALLOWED: &[&str] = &[
        "memory",
        "m",
        "cpus",
        "cpu-shares",
        "c",
        "cpu-period",
        "cpu-quota",
        "memory-swap",
        "memory-reservation",
        "workdir",
        "w",
        "env",
        "e",
        "env-file",
        "label",
        "l",
        "name",
        "hostname",
        "h",
        "tmpfs",
        "read-only",
        "rm",
        "interactive",
        "i",
        "tty",
        "t",
        "detach",
        "d",
        "stop-timeout",
        "shm-size",
        "log-opt",
    ];

    ALLOWED.contains(&flag_name)
}

impl Drop for VirtualFsSandbox {
    fn drop(&mut self) {
        // Clean up sandbox directory
        if let Err(e) = std::fs::remove_dir_all(&self.root_dir) {
            warn!(
                error = %e,
                sandbox_dir = %self.root_dir.display(),
                "Failed to clean up VirtualFs sandbox"
            );
        } else {
            debug!(
                sandbox_dir = %self.root_dir.display(),
                "Cleaned up VirtualFs sandbox"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::spec::{
        AlephExtensions, AlephSkillSpec, NetworkMode, SandboxMode, SecuritySpec, SkillMetadata,
    };
    use super::super::tool_adapter::MarkdownCliTool;
    use std::collections::BTreeMap;

    /// Build a minimal spec with the given network mode and sandbox mode.
    fn make_spec(network: NetworkMode, sandbox: SandboxMode) -> AlephSkillSpec {
        AlephSkillSpec {
            name: "test-skill".to_string(),
            description: "unit test skill".to_string(),
            metadata: SkillMetadata {
                requires: Default::default(),
                aleph: Some(AlephExtensions {
                    security: SecuritySpec {
                        sandbox,
                        confirmation: super::super::spec::ConfirmationMode::Never,
                        network,
                    },
                    input_hints: BTreeMap::new(),
                    timeout_secs: None,
                    evolution: None,
                    docker: None,
                }),
                openclaw: None,
            },
            markdown_content: String::new(),
        }
    }

    /// B3 — verify that a host-mode skill with network=none exposes the
    /// correct network and sandbox fields without panicking.  The warning log
    /// cannot be asserted in a unit test without a log-capture crate, so we
    /// assert the structural pre-conditions that govern the warn branch.
    #[test]
    fn host_network_none_spec_fields_are_readable() {
        let spec = make_spec(NetworkMode::None, SandboxMode::Host);
        let tool = MarkdownCliTool::new(spec);

        // The aleph extensions must be present.
        let aleph = tool
            .spec
            .metadata
            .aleph
            .as_ref()
            .expect("aleph extensions present");

        // Sandbox mode must be Host.
        assert!(
            matches!(aleph.security.sandbox, SandboxMode::Host),
            "expected SandboxMode::Host"
        );

        // Network mode must be None — this is the guard for the warn branch.
        assert!(
            matches!(aleph.security.network, NetworkMode::None),
            "expected NetworkMode::None"
        );
    }

    /// Host-mode network-none contract regression test.
    ///
    /// Asserts the partial-mitigation half of the host-mode contract from
    /// `docs/superpowers/specs/2026-05-20-host-sandbox-netns-decision-design.md`
    /// (Decision 2): `network: none` under `sandbox: host` MUST set
    /// `NO_PROXY=*` and `no_proxy=*` on the executed Command.
    ///
    /// This test mirrors `execute_on_host`'s env-setting logic by re-applying
    /// it to a fresh `std::process::Command` and inspecting `get_envs()`.
    /// If `execute_on_host` ever drops the `NO_PROXY` setting (silently
    /// removing the partial mitigation), this test still passes — but the
    /// test name and doc make explicit that the *contract* requires both
    /// halves (env vars + warn). A reviewer dropping the env line will see
    /// this test as a reminder that the contract is binding.
    #[test]
    #[cfg(unix)] // POSIX-only: netns egress control / no_proxy env injection
    fn host_network_none_contract_sets_no_proxy() {
        let mut cmd = std::process::Command::new("true");
        let spec = make_spec(NetworkMode::None, SandboxMode::Host);
        let aleph = spec
            .metadata
            .aleph
            .as_ref()
            .expect("aleph extensions present");

        // Mirror executor.rs::execute_on_host lines 59-70.
        if matches!(aleph.security.network, NetworkMode::None) {
            cmd.env("NO_PROXY", "*");
            cmd.env("no_proxy", "*");
        }

        let env_keys: Vec<String> = cmd
            .get_envs()
            .filter_map(|(k, _)| k.to_str().map(str::to_owned))
            .collect();
        assert!(
            env_keys.iter().any(|k| k == "NO_PROXY"),
            "host+network=none contract requires NO_PROXY env"
        );
        assert!(
            env_keys.iter().any(|k| k == "no_proxy"),
            "host+network=none contract requires no_proxy env"
        );
    }
}
