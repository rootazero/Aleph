//! Bootstrap module: detects and installs the browser runtime stack.
//!
//! Runtime components (in dependency order):
//!   1. fnm            — Node version manager (https://github.com/Schniz/fnm)
//!   2. Node.js LTS    — JavaScript runtime (managed by fnm)
//!   3. @playwright/cli — CLI binary (installed via npm)
//!   4. Chromium       — browser binary (installed via `playwright install`)
//!   5. Skills         — `~/.aleph/skills/playwright-cli/`

use std::path::PathBuf;
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::process::Command;

/// Status of one runtime component.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ComponentStatus {
    Installed {
        version: Option<String>,
        path: Option<String>,
    },
    Missing,
    Probing,
    Error {
        message: String,
    },
}

/// Combined runtime status snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapStatus {
    pub fnm: ComponentStatus,
    pub node: ComponentStatus,
    pub playwright_cli: ComponentStatus,
    pub chromium: ComponentStatus,
    pub skills: ComponentStatus,
}

impl BootstrapStatus {
    /// Probe every component without installing anything.
    /// Never panics; never blocks longer than a few seconds.
    pub async fn probe() -> Self {
        let fnm = probe_fnm().await;
        let node = match &fnm {
            ComponentStatus::Installed { .. } => probe_node().await,
            _ => ComponentStatus::Missing,
        };
        let playwright_cli = match (&fnm, &node) {
            (ComponentStatus::Installed { .. }, ComponentStatus::Installed { .. }) => {
                probe_playwright_cli().await
            }
            _ => ComponentStatus::Missing,
        };
        let chromium = match &playwright_cli {
            ComponentStatus::Installed { .. } => probe_chromium().await,
            _ => ComponentStatus::Missing,
        };
        let skills = probe_skills();
        Self { fnm, node, playwright_cli, chromium, skills }
    }
}

async fn probe_fnm() -> ComponentStatus {
    match which::which("fnm") {
        Ok(path) => {
            let version = run_capture(&path, &["--version"]).await.ok();
            ComponentStatus::Installed {
                version: version.map(|v| v.trim().to_string()),
                path: Some(path.to_string_lossy().to_string()),
            }
        }
        Err(_) => ComponentStatus::Missing,
    }
}

async fn probe_node() -> ComponentStatus {
    match run_fnm_exec(&["node", "--version"]).await {
        Ok(ver) => ComponentStatus::Installed {
            version: Some(ver.trim().to_string()),
            path: None,
        },
        Err(_) => ComponentStatus::Missing,
    }
}

async fn probe_playwright_cli() -> ComponentStatus {
    match run_fnm_exec(&["playwright-cli", "--version"]).await {
        Ok(ver) => {
            let path = run_fnm_exec(&["which", "playwright-cli"])
                .await
                .ok()
                .map(|p| p.trim().to_string());
            ComponentStatus::Installed {
                version: Some(ver.trim().to_string()),
                path,
            }
        }
        Err(_) => ComponentStatus::Missing,
    }
}

async fn probe_chromium() -> ComponentStatus {
    // playwright install --dry-run exits 0 if chromium is present.
    match run_fnm_exec(&["playwright", "install", "--dry-run", "chromium"]).await {
        Ok(stdout) if !stdout.to_lowercase().contains("missing") => ComponentStatus::Installed {
            version: None,
            path: None,
        },
        _ => ComponentStatus::Missing,
    }
}

fn probe_skills() -> ComponentStatus {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return ComponentStatus::Missing,
    };
    let path = home.join(".aleph/skills/playwright-cli");
    if path.exists() && path.is_dir() {
        let has_content = std::fs::read_dir(&path)
            .map(|mut it| it.next().is_some())
            .unwrap_or(false);
        if has_content {
            return ComponentStatus::Installed {
                version: None,
                path: Some(path.to_string_lossy().to_string()),
            };
        }
    }
    ComponentStatus::Missing
}

/// Run `fnm exec --using lts -- <args>` and capture stdout.
async fn run_fnm_exec(args: &[&str]) -> std::io::Result<String> {
    let mut full = vec!["exec", "--using", "lts", "--"];
    full.extend(args);
    let output = Command::new("fnm")
        .args(&full)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;
    if !output.status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

async fn run_capture(bin: &PathBuf, args: &[&str]) -> std::io::Result<String> {
    let output = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;
    if !output.status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// A single install step that can be executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallStep {
    Fnm,
    Node,
    PlaywrightCli,
    Chromium,
    Skills,
}

impl InstallStep {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Fnm => "fnm",
            Self::Node => "node",
            Self::PlaywrightCli => "playwright_cli",
            Self::Chromium => "chromium",
            Self::Skills => "skills",
        }
    }

    pub const ORDER: &'static [InstallStep] = &[
        Self::Fnm,
        Self::Node,
        Self::PlaywrightCli,
        Self::Chromium,
        Self::Skills,
    ];
}

/// Callback for streaming install progress.
/// Invocations: `on_progress(step, "started" | "log" | "done" | "failed", line_or_err)`.
pub type ProgressFn = std::sync::Arc<dyn Fn(InstallStep, &str, Option<String>) + Send + Sync>;

/// Run a full install of all missing components. Idempotent — skips installed ones.
pub async fn install_missing(on_progress: ProgressFn) -> Result<BootstrapStatus, String> {
    let status = BootstrapStatus::probe().await;
    for step in InstallStep::ORDER {
        let needs_install = match step {
            InstallStep::Fnm => matches!(status.fnm, ComponentStatus::Missing),
            InstallStep::Node => matches!(status.node, ComponentStatus::Missing),
            InstallStep::PlaywrightCli => matches!(status.playwright_cli, ComponentStatus::Missing),
            InstallStep::Chromium => matches!(status.chromium, ComponentStatus::Missing),
            InstallStep::Skills => matches!(status.skills, ComponentStatus::Missing),
        };
        if !needs_install {
            on_progress(*step, "done", Some("already installed".into()));
            continue;
        }
        on_progress(*step, "started", None);
        let result = match step {
            InstallStep::Fnm => install_fnm(on_progress.clone()).await,
            InstallStep::Node => install_node(on_progress.clone()).await,
            InstallStep::PlaywrightCli => install_playwright_cli(on_progress.clone()).await,
            InstallStep::Chromium => install_chromium(on_progress.clone()).await,
            InstallStep::Skills => install_skills(on_progress.clone()).await,
        };
        match result {
            Ok(_) => on_progress(*step, "done", None),
            Err(e) => {
                on_progress(*step, "failed", Some(e.clone()));
                return Err(format!("step {} failed: {e}", step.as_str()));
            }
        }
    }
    Ok(BootstrapStatus::probe().await)
}

async fn install_fnm(on_progress: ProgressFn) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let _ = on_progress;
        return Err("Windows auto-install of fnm not supported. Please run `winget install Schniz.fnm` manually.".into());
    }
    #[cfg(not(target_os = "windows"))]
    {
        on_progress(InstallStep::Fnm, "log", Some("downloading fnm installer…".into()));
        let sh_cmd = r#"curl -fsSL https://fnm.vercel.app/install | bash -s -- --skip-shell"#;
        let output = tokio::process::Command::new("bash")
            .args(["-c", sh_cmd])
            .output()
            .await
            .map_err(|e| format!("spawn bash: {e}"))?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).to_string());
        }
        on_progress(
            InstallStep::Fnm,
            "log",
            Some(String::from_utf8_lossy(&output.stdout).to_string()),
        );
        Ok(())
    }
}

async fn install_node(on_progress: ProgressFn) -> Result<(), String> {
    on_progress(InstallStep::Node, "log", Some("fnm install --lts".into()));
    let output = tokio::process::Command::new("fnm")
        .args(["install", "--lts"])
        .output()
        .await
        .map_err(|e| format!("spawn fnm: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    Ok(())
}

async fn install_playwright_cli(_on_progress: ProgressFn) -> Result<(), String> {
    let output = tokio::process::Command::new("fnm")
        .args(["exec", "--using", "lts", "--", "npm", "install", "-g", "@playwright/cli@latest"])
        .output()
        .await
        .map_err(|e| format!("spawn fnm: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    Ok(())
}

async fn install_chromium(_on_progress: ProgressFn) -> Result<(), String> {
    let output = tokio::process::Command::new("fnm")
        .args(["exec", "--using", "lts", "--", "playwright", "install", "chromium"])
        .output()
        .await
        .map_err(|e| format!("spawn fnm: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    Ok(())
}

async fn install_skills(_on_progress: ProgressFn) -> Result<(), String> {
    let home = dirs::home_dir().ok_or_else(|| "no home dir".to_string())?;
    let skills_target = home.join(".aleph/skills/playwright-cli");
    tokio::fs::create_dir_all(&skills_target)
        .await
        .map_err(|e| e.to_string())?;
    let target_str = skills_target.to_string_lossy().to_string();
    // Try `--target <path>` first; fall back to default install if CLI rejects the flag.
    let with_target = tokio::process::Command::new("fnm")
        .args(["exec", "--using", "lts", "--", "playwright-cli", "install", "--skills", "--target", &target_str])
        .output()
        .await
        .map_err(|e| format!("spawn fnm: {e}"))?;
    if with_target.status.success() {
        return Ok(());
    }
    let default_install = tokio::process::Command::new("fnm")
        .args(["exec", "--using", "lts", "--", "playwright-cli", "install", "--skills"])
        .output()
        .await
        .map_err(|e| format!("spawn fnm: {e}"))?;
    if !default_install.status.success() {
        return Err(String::from_utf8_lossy(&default_install.stderr).to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_component_status_serde() {
        let s = ComponentStatus::Installed { version: Some("v22.8.0".into()), path: None };
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["state"], "installed");
        assert_eq!(j["version"], "v22.8.0");
    }

    #[test]
    fn test_component_status_missing_serializes_tag() {
        let s = ComponentStatus::Missing;
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["state"], "missing");
    }

    #[tokio::test]
    async fn test_probe_skills_no_panic() {
        let _status = probe_skills();
    }

    #[tokio::test]
    async fn test_probe_completes_without_panicking() {
        let status = BootstrapStatus::probe().await;
        let _ = serde_json::to_value(&status).unwrap();
    }

    #[test]
    fn test_install_step_order() {
        let order: Vec<&str> = InstallStep::ORDER.iter().map(|s| s.as_str()).collect();
        assert_eq!(order, vec!["fnm", "node", "playwright_cli", "chromium", "skills"]);
    }

    #[test]
    fn test_install_step_serde() {
        let s = InstallStep::PlaywrightCli;
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j, serde_json::Value::String("playwright_cli".into()));
    }
}
