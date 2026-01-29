//! Command analysis structures.
//!
//! Represents parsed and analyzed shell commands for security decisions.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Result of analyzing a shell command
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandAnalysis {
    /// Whether the command was successfully parsed
    pub ok: bool,

    /// Error reason if parsing failed
    pub reason: Option<String>,

    /// All command segments (flattened from all chains/pipelines)
    pub segments: Vec<CommandSegment>,

    /// Commands grouped by chain operators (&&, ||, ;)
    /// Each inner Vec is a pipeline (commands connected by |)
    pub chains: Option<Vec<Vec<CommandSegment>>>,
}

impl CommandAnalysis {
    /// Create a successful analysis with segments
    pub fn success(segments: Vec<CommandSegment>, chains: Vec<Vec<CommandSegment>>) -> Self {
        Self {
            ok: true,
            reason: None,
            segments,
            chains: Some(chains),
        }
    }

    /// Create a failed analysis with error reason
    pub fn error(reason: impl Into<String>) -> Self {
        Self {
            ok: false,
            reason: Some(reason.into()),
            segments: Vec::new(),
            chains: None,
        }
    }

    /// Get all executable names from segments
    pub fn executables(&self) -> Vec<&str> {
        self.segments
            .iter()
            .filter_map(|s| s.resolution.as_ref())
            .map(|r| r.executable_name.as_str())
            .collect()
    }
}

/// A single command segment (one command in a pipeline)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandSegment {
    /// Raw command string
    pub raw: String,

    /// Tokenized arguments
    pub argv: Vec<String>,

    /// Resolution information (if executable was found)
    pub resolution: Option<CommandResolution>,
}

impl CommandSegment {
    /// Create a new segment with raw string and argv
    pub fn new(raw: impl Into<String>, argv: Vec<String>) -> Self {
        Self {
            raw: raw.into(),
            argv,
            resolution: None,
        }
    }

    /// Set resolution
    pub fn with_resolution(mut self, resolution: CommandResolution) -> Self {
        self.resolution = Some(resolution);
        self
    }

    /// Get the executable name (first argv element)
    pub fn executable(&self) -> Option<&str> {
        self.argv.first().map(|s| s.as_str())
    }
}

/// Resolution information for an executable
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandResolution {
    /// Raw executable as specified (e.g., "git", "./script.sh", "/usr/bin/ls")
    pub raw_executable: String,

    /// Fully resolved path (if found in PATH)
    pub resolved_path: Option<PathBuf>,

    /// Executable name (basename without path)
    pub executable_name: String,
}

impl CommandResolution {
    /// Create a resolution for an executable found in PATH
    pub fn found(raw: impl Into<String>, path: PathBuf) -> Self {
        let raw = raw.into();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&raw)
            .to_string();

        Self {
            raw_executable: raw,
            resolved_path: Some(path),
            executable_name: name,
        }
    }

    /// Create a resolution for an executable not found in PATH
    pub fn not_found(raw: impl Into<String>) -> Self {
        let raw = raw.into();
        let name = raw
            .rsplit('/')
            .next()
            .unwrap_or(&raw)
            .to_string();

        Self {
            raw_executable: raw,
            resolved_path: None,
            executable_name: name,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analysis_success() {
        let segment = CommandSegment::new("ls -la", vec!["ls".into(), "-la".into()]);
        let analysis = CommandAnalysis::success(vec![segment.clone()], vec![vec![segment]]);

        assert!(analysis.ok);
        assert!(analysis.reason.is_none());
        assert_eq!(analysis.segments.len(), 1);
    }

    #[test]
    fn test_analysis_error() {
        let analysis = CommandAnalysis::error("parse failed");

        assert!(!analysis.ok);
        assert_eq!(analysis.reason, Some("parse failed".to_string()));
    }

    #[test]
    fn test_segment_executable() {
        let segment = CommandSegment::new("git status", vec!["git".into(), "status".into()]);
        assert_eq!(segment.executable(), Some("git"));
    }

    #[test]
    fn test_resolution_found() {
        let res = CommandResolution::found("git", PathBuf::from("/usr/bin/git"));
        assert_eq!(res.executable_name, "git");
        assert!(res.resolved_path.is_some());
    }

    #[test]
    fn test_resolution_not_found() {
        let res = CommandResolution::not_found("./my-script.sh");
        assert_eq!(res.executable_name, "my-script.sh");
        assert!(res.resolved_path.is_none());
    }

    #[test]
    fn test_executables() {
        let seg1 = CommandSegment::new("ls", vec!["ls".into()])
            .with_resolution(CommandResolution::found("ls", PathBuf::from("/bin/ls")));
        let seg2 = CommandSegment::new("grep", vec!["grep".into()])
            .with_resolution(CommandResolution::found("grep", PathBuf::from("/usr/bin/grep")));

        let analysis = CommandAnalysis::success(vec![seg1, seg2], vec![]);
        let executables = analysis.executables();

        assert_eq!(executables, vec!["ls", "grep"]);
    }
}
