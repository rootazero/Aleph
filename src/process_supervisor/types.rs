//! PtySupervisor type definitions.

use std::path::PathBuf;

/// PTY terminal size configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtySize {
    pub rows: u16,
    pub cols: u16,
}

impl Default for PtySize {
    fn default() -> Self {
        Self {
            rows: 24,
            cols: 120,
        }
    }
}

/// Supervisor configuration.
#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    /// Working directory.
    pub workspace: PathBuf,
    /// PTY terminal size.
    pub pty_size: PtySize,
    /// Command to execute (default: "claude").
    pub command: String,
    /// Command arguments.
    pub args: Vec<String>,
    /// Custom secret masking patterns (regex, replacement).
    pub custom_secret_patterns: Vec<(String, String)>,
}

/// Characters disallowed in command arguments to prevent shell injection.
const ARGUMENT_FORBIDDEN_CHARS: &[char] = &[';', '|', '&', '$', '`', '(', ')', '<', '>', '\n'];

impl SupervisorConfig {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            pty_size: PtySize::default(),
            command: "claude".to_string(),
            args: vec![],
            custom_secret_patterns: vec![],
        }
    }

    pub fn with_command(mut self, cmd: impl Into<String>) -> Self {
        self.command = cmd.into();
        self
    }

    /// Set command arguments. Rejects strings containing shell metacharacters
    /// to mitigate command injection risks.
    pub fn with_args(mut self, args: Vec<String>) -> Result<Self, SupervisorError> {
        for arg in &args {
            if let Some(ch) = arg.chars().find(|c| ARGUMENT_FORBIDDEN_CHARS.contains(c)) {
                return Err(SupervisorError::InvalidArgument(format!(
                    "argument contains forbidden character '{}': {}",
                    ch, arg
                )));
            }
        }
        self.args = args;
        Ok(self)
    }

    pub fn with_pty_size(mut self, rows: u16, cols: u16) -> Self {
        self.pty_size = PtySize { rows, cols };
        self
    }

    /// Add a custom secret masking pattern.
    pub fn with_secret_pattern(
        mut self,
        regex: impl Into<String>,
        replacement: impl Into<String>,
    ) -> Self {
        self.custom_secret_patterns
            .push((regex.into(), replacement.into()));
        self
    }
}

/// Supervisor event types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupervisorEvent {
    /// Received output line.
    Output(String),
    /// Process exited.
    Exited(i32),
    /// Detected approval request.
    ApprovalRequest(String),
    /// Detected context window overflow.
    ContextOverflow,
    /// Detected error.
    Error(String),
}

/// Supervisor error types.
#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    #[error("Failed to create PTY: {0}")]
    PtyCreation(String),
    #[error("Failed to spawn command: {0}")]
    SpawnFailed(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Process not running")]
    NotRunning,
    #[error("Write failed: {0}")]
    WriteFailed(String),
    #[error("Invalid argument: {0}")]
    InvalidArgument(String),
}
