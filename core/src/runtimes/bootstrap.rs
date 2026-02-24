//! Shell-driven bootstrap for runtime installation
//!
//! Installation logic is embedded as shell scripts, not Rust code.

use crate::error::AlephError;
use std::path::PathBuf;
use std::process::Command;

/// Result of a bootstrap attempt
#[derive(Debug)]
pub enum BootstrapResult {
    Success { bin_path: PathBuf },
    PathNotFound { expected: PathBuf },
    Failed { stderr: String },
}

struct BootstrapSpec {
    capability: &'static str,
    script_macos: &'static str,
    script_linux: &'static str,
    expected_paths: &'static [&'static str],
}

const BOOTSTRAP_SPECS: &[BootstrapSpec] = &[
    BootstrapSpec {
        capability: "uv",
        script_macos: "curl -LsSf https://astral.sh/uv/install.sh | sh",
        script_linux: "curl -LsSf https://astral.sh/uv/install.sh | sh",
        expected_paths: &["~/.local/bin/uv", "~/.cargo/bin/uv"],
    },
    BootstrapSpec {
        capability: "python",
        script_macos: "~/.local/bin/uv python install 3.12 && ~/.local/bin/uv venv ~/.aleph/runtimes/python/default --python 3.12",
        script_linux: "~/.local/bin/uv python install 3.12 && ~/.local/bin/uv venv ~/.aleph/runtimes/python/default --python 3.12",
        expected_paths: &["~/.aleph/runtimes/python/default/bin/python3"],
    },
    BootstrapSpec {
        capability: "node",
        script_macos: "curl -fsSL https://fnm.vercel.app/install | bash -s -- --skip-shell && eval \"$(~/.local/share/fnm/fnm env)\" && ~/.local/share/fnm/fnm install --lts",
        script_linux: "curl -fsSL https://fnm.vercel.app/install | bash -s -- --skip-shell && eval \"$(~/.local/share/fnm/fnm env)\" && ~/.local/share/fnm/fnm install --lts",
        expected_paths: &["~/.local/share/fnm/aliases/default/bin/node", "~/.fnm/aliases/default/bin/node"],
    },
    BootstrapSpec {
        capability: "ffmpeg",
        script_macos: "brew install ffmpeg 2>/dev/null || echo 'Please install ffmpeg manually: brew install ffmpeg'",
        script_linux: "sudo apt-get install -y ffmpeg 2>/dev/null || echo 'Please install ffmpeg manually'",
        expected_paths: &["/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg", "/usr/bin/ffmpeg"],
    },
    BootstrapSpec {
        capability: "yt-dlp",
        script_macos: "~/.local/bin/uv tool install yt-dlp",
        script_linux: "~/.local/bin/uv tool install yt-dlp",
        expected_paths: &["~/.local/bin/yt-dlp"],
    },
];

/// Dependencies that must be bootstrapped first
pub fn dependencies(capability: &str) -> &'static [&'static str] {
    match capability {
        "python" => &["uv"],
        "yt-dlp" => &["uv"],
        _ => &[],
    }
}

/// Execute bootstrap for a capability via shell
pub fn bootstrap(capability: &str) -> Result<BootstrapResult, AlephError> {
    let spec = BOOTSTRAP_SPECS
        .iter()
        .find(|s| s.capability == capability)
        .ok_or_else(|| AlephError::runtime(capability, "No bootstrap spec found"))?;

    let script = if cfg!(target_os = "macos") {
        spec.script_macos
    } else {
        spec.script_linux
    };

    tracing::info!("Bootstrapping {} via shell...", capability);

    let output = Command::new("sh")
        .arg("-c")
        .arg(script)
        .output()
        .map_err(|e| AlephError::runtime(capability, &format!("Failed to execute bootstrap: {}", e)))?;

    if output.status.success() {
        for expected in spec.expected_paths {
            let expanded = expand_tilde(expected);
            if expanded.exists() {
                tracing::info!("Bootstrap {} succeeded: {}", capability, expanded.display());
                return Ok(BootstrapResult::Success { bin_path: expanded });
            }
        }
        let expected = expand_tilde(spec.expected_paths[0]);
        Ok(BootstrapResult::PathNotFound { expected })
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        tracing::warn!("Bootstrap {} failed: {}", capability, stderr);
        Ok(BootstrapResult::Failed { stderr })
    }
}

/// Has a bootstrap spec for this capability
pub fn has_spec(capability: &str) -> bool {
    BOOTSTRAP_SPECS.iter().any(|s| s.capability == capability)
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dependencies() {
        assert_eq!(dependencies("python"), &["uv"]);
        assert_eq!(dependencies("yt-dlp"), &["uv"]);
        assert!(dependencies("ffmpeg").is_empty());
        assert!(dependencies("uv").is_empty());
    }

    #[test]
    fn test_has_spec() {
        assert!(has_spec("uv"));
        assert!(has_spec("python"));
        assert!(has_spec("node"));
        assert!(has_spec("ffmpeg"));
        assert!(has_spec("yt-dlp"));
        assert!(!has_spec("ruby"));
    }

    #[test]
    fn test_expand_tilde() {
        let expanded = expand_tilde("~/.local/bin/uv");
        assert!(!expanded.to_string_lossy().contains('~'));
        assert!(expanded.to_string_lossy().contains(".local/bin/uv"));
    }

    #[test]
    fn test_expand_tilde_absolute() {
        let expanded = expand_tilde("/usr/local/bin/ffmpeg");
        assert_eq!(expanded, PathBuf::from("/usr/local/bin/ffmpeg"));
    }
}
