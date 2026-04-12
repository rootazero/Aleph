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
}
