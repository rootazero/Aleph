//! Runtime Capability Module
//!
//! Provides runtime capability descriptions for injection into AI system prompts.
//! This allows the AI to understand which runtimes are available and how to use them.

use super::registry::RuntimeRegistry;
use super::RuntimeInfo;
use std::path::PathBuf;

/// Runtime capability description for AI system prompt injection
#[derive(Debug, Clone)]
pub struct RuntimeCapability {
    /// Unique identifier (e.g., "yt-dlp", "uv", "fnm")
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Description of what this runtime provides
    pub description: String,
    /// Path to the executable (if installed)
    pub executable_path: Option<PathBuf>,
    /// Whether the runtime is installed
    pub installed: bool,
    /// Installed version (if available)
    pub version: Option<String>,
}

impl RuntimeCapability {
    /// Create capability from RuntimeInfo and executable path
    pub fn from_info(info: &RuntimeInfo, executable_path: PathBuf) -> Self {
        Self {
            id: info.id.to_string(),
            name: info.name.to_string(),
            description: info.description.to_string(),
            executable_path: if info.installed {
                Some(executable_path)
            } else {
                None
            },
            installed: info.installed,
            version: info.version.clone(),
        }
    }

    /// Get all installed runtime capabilities from the registry
    ///
    /// Returns only the runtimes that are currently installed.
    pub fn get_installed_from_registry(registry: &RuntimeRegistry) -> Vec<RuntimeCapability> {
        let runtimes = registry.list();
        let mut capabilities = Vec::new();

        for info in runtimes {
            if info.installed {
                // Get executable path from the runtime
                if let Some(runtime) = registry.get(info.id) {
                    let capability = RuntimeCapability::from_info(&info, runtime.executable_path());
                    capabilities.push(capability);
                }
            }
        }

        capabilities
    }

    /// Format runtime capabilities for system prompt injection
    ///
    /// Generates a markdown-formatted section describing available runtimes.
    pub fn format_for_prompt(capabilities: &[RuntimeCapability]) -> String {
        if capabilities.is_empty() {
            return String::new();
        }

        let mut output = String::new();
        output.push_str("You can execute code using these installed runtimes:\n\n");

        for cap in capabilities {
            output.push_str(&format!("**{}**\n", cap.name));

            // Add description
            output.push_str(&format!("- {}\n", cap.description));

            // Add version if available
            if let Some(ref version) = cap.version {
                output.push_str(&format!("- Version: {}\n", version));
            }

            // Add executable path
            if let Some(ref path) = cap.executable_path {
                output.push_str(&format!("- Executable: `{}`\n", path.display()));
            }

            // Add usage hints based on runtime type
            output.push_str(&Self::get_usage_hints(&cap.id));
            output.push('\n');
        }

        output
    }

    /// Get usage hints for specific runtimes
    fn get_usage_hints(runtime_id: &str) -> String {
        match runtime_id {
            "uv" => {
                "- Use for Python script execution and package management\n\
                 - Install packages: `uv pip install <package>`\n"
                    .to_string()
            }
            "fnm" => {
                "- Use for Node.js/JavaScript execution\n\
                 - Run scripts with `node` command\n\
                 - Install packages with `npm install`\n"
                    .to_string()
            }
            "ffmpeg" => {
                "- Use for audio/video processing and conversion\n\
                 - Supports most media formats\n"
                    .to_string()
            }
            "yt-dlp" => {
                "- Use for downloading videos from YouTube and other sites\n\
                 - Can extract audio, subtitles, and metadata\n"
                    .to_string()
            }
            _ => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_empty_capabilities() {
        let capabilities: Vec<RuntimeCapability> = vec![];
        let result = RuntimeCapability::format_for_prompt(&capabilities);
        assert!(result.is_empty());
    }

    #[test]
    fn test_format_single_capability() {
        let capabilities = vec![RuntimeCapability {
            id: "uv".to_string(),
            name: "uv (Python)".to_string(),
            description: "Python package manager".to_string(),
            executable_path: Some(PathBuf::from("/path/to/python")),
            installed: true,
            version: Some("3.11.0".to_string()),
        }];

        let result = RuntimeCapability::format_for_prompt(&capabilities);

        assert!(result.contains("uv (Python)"));
        assert!(result.contains("Python package manager"));
        assert!(result.contains("3.11.0"));
        assert!(result.contains("/path/to/python"));
    }

    #[test]
    fn test_usage_hints() {
        assert!(RuntimeCapability::get_usage_hints("uv").contains("Python"));
        assert!(RuntimeCapability::get_usage_hints("fnm").contains("Node.js"));
        assert!(RuntimeCapability::get_usage_hints("ffmpeg").contains("audio/video"));
        assert!(RuntimeCapability::get_usage_hints("yt-dlp").contains("YouTube"));
        assert!(RuntimeCapability::get_usage_hints("unknown").is_empty());
    }
}
