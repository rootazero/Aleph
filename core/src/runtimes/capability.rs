//! Runtime Capability Module
//!
//! Provides runtime capability descriptions for injection into AI system prompts.
//! This allows the AI to understand which runtimes are available and how to use them.

use super::ledger::CapabilityEntry;
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
        get_usage_hints(runtime_id)
    }
}

/// Get usage hints for a runtime by its identifier.
///
/// Returns markdown-formatted hints describing how to use the runtime.
/// Standalone function so it can be called from both `RuntimeCapability`
/// methods and `format_entries_for_prompt`.
fn get_usage_hints(runtime_id: &str) -> String {
    match runtime_id {
        "uv" => {
            "- Use for Python script execution and package management\n\
             - Install packages: `uv pip install <package>`\n"
                .to_string()
        }
        "fnm" | "node" => {
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

/// Format capability entries for AI system prompt injection.
///
/// Takes a slice of `&CapabilityEntry` references (as returned by
/// `CapabilityLedger::list_ready()`) and produces markdown text
/// suitable for inclusion in the system prompt.
pub fn format_entries_for_prompt(entries: &[&CapabilityEntry]) -> String {
    if entries.is_empty() {
        return String::new();
    }

    let mut output = String::new();
    for entry in entries {
        output.push_str(&format!("**{}**\n", entry.name));
        if !entry.version.is_empty() {
            output.push_str(&format!("- Version: {}\n", entry.version));
        }
        if !entry.bin_path.as_os_str().is_empty() {
            output.push_str(&format!("- Executable: {}\n", entry.bin_path.display()));
        }
        // Reuse existing usage hints
        let hints = get_usage_hints(&entry.name);
        if !hints.is_empty() {
            output.push_str(&hints);
        }
        output.push('\n');
    }
    output
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
        assert!(RuntimeCapability::get_usage_hints("node").contains("Node.js"));
        assert!(RuntimeCapability::get_usage_hints("ffmpeg").contains("audio/video"));
        assert!(RuntimeCapability::get_usage_hints("yt-dlp").contains("YouTube"));
        assert!(RuntimeCapability::get_usage_hints("unknown").is_empty());
    }

    // -- format_entries_for_prompt tests -----------------------------------

    #[test]
    fn test_format_entries_for_prompt_empty() {
        let entries: Vec<&CapabilityEntry> = vec![];
        let result = format_entries_for_prompt(&entries);
        assert!(result.is_empty());
    }

    #[test]
    fn test_format_entries_for_prompt_single() {
        let entry = CapabilityEntry {
            name: "uv".to_string(),
            bin_path: PathBuf::from("/home/user/.aleph/runtimes/uv/bin/uv"),
            version: "0.5.14".to_string(),
            status: super::super::ledger::CapabilityStatus::Ready,
            source: super::super::ledger::CapabilitySource::AlephManaged,
            last_probed: 0,
        };
        let entries: Vec<&CapabilityEntry> = vec![&entry];
        let result = format_entries_for_prompt(&entries);

        assert!(result.contains("**uv**"), "should contain bold name");
        assert!(result.contains("Version: 0.5.14"), "should contain version");
        assert!(result.contains("/home/user/.aleph/runtimes/uv/bin/uv"), "should contain bin path");
        assert!(result.contains("Python"), "should contain usage hints for uv");
    }

    #[test]
    fn test_format_entries_for_prompt_no_version() {
        let entry = CapabilityEntry {
            name: "ffmpeg".to_string(),
            bin_path: PathBuf::from("/usr/bin/ffmpeg"),
            version: String::new(),
            status: super::super::ledger::CapabilityStatus::Ready,
            source: super::super::ledger::CapabilitySource::System,
            last_probed: 0,
        };
        let entries: Vec<&CapabilityEntry> = vec![&entry];
        let result = format_entries_for_prompt(&entries);

        assert!(result.contains("**ffmpeg**"));
        assert!(!result.contains("Version:"), "empty version should be omitted");
        assert!(result.contains("audio/video"), "should contain usage hints for ffmpeg");
    }

    #[test]
    fn test_format_entries_for_prompt_unknown_runtime() {
        let entry = CapabilityEntry {
            name: "custom-tool".to_string(),
            bin_path: PathBuf::from("/opt/bin/custom-tool"),
            version: "2.0".to_string(),
            status: super::super::ledger::CapabilityStatus::Ready,
            source: super::super::ledger::CapabilitySource::System,
            last_probed: 0,
        };
        let entries: Vec<&CapabilityEntry> = vec![&entry];
        let result = format_entries_for_prompt(&entries);

        assert!(result.contains("**custom-tool**"));
        assert!(result.contains("Version: 2.0"));
        // No usage hints for unknown runtimes — just name/version/path
    }
}
