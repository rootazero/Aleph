//! Runtime Context - Micro-environmental awareness for prompt injection
//!
//! Collects lightweight runtime environment metadata (OS, arch, shell, working directory,
//! current model, hostname) and formats it as a compact prompt section. This gives the
//! LLM grounding in the physical execution environment without heavy dependencies.
//!
//! # Usage
//!
//! ```rust,no_run
//! use alephcore::thinker::RuntimeContext;
//!
//! let ctx = RuntimeContext::collect("claude-opus-4-6");
//! let section = ctx.to_prompt_section();
//! // => "## Runtime Environment\nos=macos | arch=aarch64 | shell=zsh | cwd=/workspace | model=claude-opus-4-6 | host=MacBook-Pro"
//! ```

use std::path::PathBuf;

/// Lightweight snapshot of the runtime environment.
///
/// Collected once at prompt-build time and injected into the system prompt
/// so the LLM knows where it is running.
#[derive(Debug, Clone)]
pub struct RuntimeContext {
    /// Operating system name, e.g. "macos", "linux", "windows"
    pub os: String,
    /// CPU architecture, e.g. "aarch64", "x86_64"
    pub arch: String,
    /// User's default shell, e.g. "zsh", "bash"
    pub shell: String,
    /// Current working directory
    pub working_dir: PathBuf,
    /// Git repository root, if inside a repo (caller provides from cached git info)
    pub repo_root: Option<PathBuf>,
    /// Current LLM model identifier
    pub current_model: String,
    /// Machine hostname
    pub hostname: String,
    /// Current local time as human-readable string, e.g. "2026-03-30 14:30:00"
    pub current_time: String,
    /// Current time in milliseconds since epoch (UTC), for precise scheduling
    pub current_time_ms: i64,
    /// Local timezone identifier, e.g. "Asia/Shanghai", "UTC+8"
    pub timezone: String,
}

impl RuntimeContext {
    /// Collect runtime context from the current environment.
    ///
    /// `current_model` is passed in because the caller (prompt builder) knows which
    /// model was selected by the router.
    ///
    /// `repo_root` is left as `None` — the caller should set it from cached git info
    /// after calling this method.
    pub fn collect(current_model: &str) -> Self {
        let os = std::env::consts::OS.to_string();
        let arch = std::env::consts::ARCH.to_string();

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "unknown".to_string());

        let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));

        let hostname = std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("COMPUTERNAME"))
            .unwrap_or_else(|_| "unknown".to_string());

        // Collect current local time, epoch ms, and timezone
        let now_local = chrono::Local::now();
        let current_time = now_local.format("%Y-%m-%d %H:%M:%S").to_string();
        let current_time_ms = now_local.timestamp_millis();
        let timezone = {
            let offset = now_local.offset();
            // Try to get IANA timezone from TZ env var, fall back to UTC offset
            std::env::var("TZ").unwrap_or_else(|_| {
                let secs = offset.local_minus_utc();
                let hours = secs / 3600;
                let mins = (secs.abs() % 3600) / 60;
                if mins == 0 {
                    format!("UTC{:+}", hours)
                } else {
                    format!("UTC{:+}:{:02}", hours, mins)
                }
            })
        };

        Self {
            os,
            arch,
            shell,
            working_dir,
            repo_root: None,
            current_model: current_model.to_string(),
            hostname,
            current_time,
            current_time_ms,
            timezone,
        }
    }

    /// Format as a compact prompt section for system prompt injection.
    ///
    /// Output example (with repo):
    /// ```text
    /// ## Runtime Environment
    /// os=macos | arch=aarch64 | shell=zsh | cwd=/workspace | repo=/workspace | model=claude-opus-4-6 | host=MacBook-Pro
    /// ```
    ///
    /// The `repo=` segment is omitted when `repo_root` is `None`.
    pub fn to_prompt_section(&self) -> String {
        let mut parts = vec![
            format!("os={}", self.os),
            format!("arch={}", self.arch),
            format!("shell={}", self.shell),
            format!("cwd={}", self.working_dir.display()),
        ];

        if let Some(ref repo) = self.repo_root {
            parts.push(format!("repo={}", repo.display()));
        }

        parts.push(format!("model={}", self.current_model));
        parts.push(format!("host={}", self.hostname));
        parts.push(format!("time={} ({})", self.current_time, self.timezone));
        parts.push(format!("time_ms={}", self.current_time_ms));

        format!("## Runtime Environment\n{}", parts.join(" | "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_returns_valid_context() {
        let ctx = RuntimeContext::collect("test-model-v1");

        // OS and arch should come from std::env::consts (never empty)
        assert!(!ctx.os.is_empty(), "os should not be empty");
        assert!(!ctx.arch.is_empty(), "arch should not be empty");

        // Shell should be populated (at least "unknown" as fallback)
        assert!(!ctx.shell.is_empty(), "shell should not be empty");

        // Working dir should be a valid path
        assert!(
            ctx.working_dir.to_str().is_some(),
            "working_dir should be a valid UTF-8 path"
        );

        // repo_root defaults to None
        assert!(ctx.repo_root.is_none(), "repo_root should default to None");

        // current_model should match what we passed in
        assert_eq!(ctx.current_model, "test-model-v1");

        // hostname should be populated (at least "unknown" as fallback)
        assert!(!ctx.hostname.is_empty(), "hostname should not be empty");
    }

    fn make_test_ctx(repo_root: Option<PathBuf>) -> RuntimeContext {
        RuntimeContext {
            os: "macos".to_string(),
            arch: "aarch64".to_string(),
            shell: "zsh".to_string(),
            working_dir: PathBuf::from("/workspace"),
            repo_root,
            current_model: "claude-opus-4-6".to_string(),
            hostname: "MacBook-Pro".to_string(),
            current_time: "2026-03-30 14:30:00".to_string(),
            current_time_ms: 1774852200000,
            timezone: "Asia/Shanghai".to_string(),
        }
    }

    #[test]
    fn test_to_prompt_section_format() {
        let ctx = make_test_ctx(Some(PathBuf::from("/workspace")));

        let section = ctx.to_prompt_section();

        assert!(section.starts_with("## Runtime Environment\n"));
        assert!(section.contains("os=macos"));
        assert!(section.contains("arch=aarch64"));
        assert!(section.contains("shell=zsh"));
        assert!(section.contains("cwd=/workspace"));
        assert!(section.contains("repo=/workspace"));
        assert!(section.contains("model=claude-opus-4-6"));
        assert!(section.contains("host=MacBook-Pro"));
        assert!(section.contains("time=2026-03-30 14:30:00 (Asia/Shanghai)"));
        assert!(section.contains("time_ms=1774852200000"));

        // Verify pipe-separated format on the data line
        let lines: Vec<&str> = section.lines().collect();
        assert_eq!(lines.len(), 2, "should have header + data line");
        assert_eq!(lines[0], "## Runtime Environment");
        assert!(
            lines[1].contains(" | "),
            "data line should use pipe separators"
        );
    }

    #[test]
    fn test_to_prompt_section_no_repo() {
        let ctx = RuntimeContext {
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            shell: "bash".to_string(),
            working_dir: PathBuf::from("/home/user"),
            repo_root: None,
            current_model: "gpt-4".to_string(),
            hostname: "server-01".to_string(),
            current_time: "2026-03-30 02:30:00".to_string(),
            current_time_ms: 1774852200000,
            timezone: "UTC".to_string(),
        };

        let section = ctx.to_prompt_section();

        assert!(section.starts_with("## Runtime Environment\n"));
        assert!(section.contains("os=linux"));
        assert!(section.contains("arch=x86_64"));
        assert!(section.contains("shell=bash"));
        assert!(section.contains("cwd=/home/user"));
        assert!(
            !section.contains("repo="),
            "should NOT contain repo= when repo_root is None"
        );
        assert!(section.contains("model=gpt-4"));
        assert!(section.contains("host=server-01"));
        assert!(section.contains("time="));
        assert!(section.contains("time_ms="));
    }
}
