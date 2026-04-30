//! PTY-based supervisor implementation.

use crate::sync_primitives::Arc;
use crate::sync_primitives::{AtomicBool, Ordering};
use portable_pty::{
    native_pty_system, Child, CommandBuilder, MasterPty, PtySize as PortablePtySize,
};
use std::io::{BufRead, BufReader, Write};
use tokio::sync::mpsc;

use crate::exec::SecretMasker;
use crate::process_supervisor::types::{SupervisorConfig, SupervisorError, SupervisorEvent};

/// PTY-based supervisor for controlling Claude Code and similar CLI tools.
///
/// # Example
///
/// ```rust,no_run
/// use alephcore::supervisor::{ClaudeSupervisor, SupervisorConfig};
///
/// let config = SupervisorConfig::new("/path/to/workspace");
/// let mut supervisor = ClaudeSupervisor::new(config);
///
/// // Spawn the process
/// let mut rx = supervisor.spawn().unwrap();
///
/// // Send input
/// supervisor.write("Hello\n").unwrap();
///
/// // Read events
/// while let Some(event) = rx.blocking_recv() {
///     println!("Event: {:?}", event);
/// }
/// ```
pub struct ClaudeSupervisor {
    config: SupervisorConfig,
    master: Option<Box<dyn MasterPty + Send>>,
    writer: Option<Box<dyn Write + Send>>,
    child: Option<Box<dyn Child + Send>>,
    reader_handle: Option<std::thread::JoinHandle<()>>,
    running: Arc<AtomicBool>,
    masker: SecretMasker,
    event_tx: Option<mpsc::UnboundedSender<SupervisorEvent>>,
}

impl ClaudeSupervisor {
    /// Create a new supervisor with the given configuration.
    pub fn new(config: SupervisorConfig) -> Self {
        let mut masker = SecretMasker::new();
        for (pattern, replacement) in &config.custom_secret_patterns {
            if let Err(e) = masker.add_pattern(pattern, replacement) {
                tracing::warn!("Failed to add custom secret pattern '{}': {}", pattern, e);
            }
        }
        Self {
            config,
            master: None,
            writer: None,
            child: None,
            reader_handle: None,
            running: Arc::new(AtomicBool::new(false)),
            masker,
            event_tx: None,
        }
    }

    /// Check if the supervised process is currently running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    /// Spawn the supervised process and return an event receiver.
    ///
    /// Returns a channel receiver that will emit `SupervisorEvent` as they occur.
    pub fn spawn(&mut self) -> Result<mpsc::UnboundedReceiver<SupervisorEvent>, SupervisorError> {
        let pty_system = native_pty_system();

        // Create PTY pair
        let pair = pty_system
            .openpty(PortablePtySize {
                rows: self.config.pty_size.rows,
                cols: self.config.pty_size.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| SupervisorError::PtyCreation(e.to_string()))?;

        // Build command
        let mut cmd = CommandBuilder::new(&self.config.command);
        cmd.cwd(&self.config.workspace);
        for arg in &self.config.args {
            cmd.arg(arg);
        }

        // Spawn process (keep child handle alive to prevent premature termination)
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| SupervisorError::SpawnFailed(e.to_string()))?;

        // Get reader and writer
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| {
                SupervisorError::Io(std::io::Error::other(format!(
                    "failed to clone PTY reader: {}",
                    e
                )))
            })?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| {
                SupervisorError::Io(std::io::Error::other(format!(
                    "failed to take PTY writer: {}",
                    e
                )))
            })?;

        self.master = Some(pair.master);
        self.writer = Some(writer);
        self.running.store(true, Ordering::Release);

        // Create event channel
        let (tx, rx) = mpsc::unbounded_channel();
        self.event_tx = Some(tx.clone());
        let running = self.running.clone();
        let masker = self.masker.clone();

        // Spawn reader thread
        let handle = std::thread::spawn(move || {
            let buf_reader = BufReader::new(reader);
            for line in buf_reader.lines() {
                match line {
                    Ok(text) => {
                        let clean = strip_ansi(&text);
                        let safe = masker.mask(&clean);
                        let event = detect_event(&safe);
                        if tx.send(event).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::debug!("PTY read error: {}", e);
                        break;
                    }
                }
            }
            running.store(false, Ordering::Release);
        });

        self.reader_handle = Some(handle);
        self.child = Some(child);

        Ok(rx)
    }

    /// Write input to the supervised process.
    pub fn write(&mut self, input: &str) -> Result<(), SupervisorError> {
        let writer = self.writer.as_mut().ok_or(SupervisorError::NotRunning)?;
        writer
            .write_all(input.as_bytes())
            .map_err(|e| SupervisorError::WriteFailed(e.to_string()))?;
        writer
            .flush()
            .map_err(|e| SupervisorError::WriteFailed(e.to_string()))?;
        Ok(())
    }

    /// Write a line (appends newline) to the supervised process.
    pub fn writeln(&mut self, input: &str) -> Result<(), SupervisorError> {
        self.write(input)?;
        self.write("\n")
    }

    /// Gracefully terminate the supervised process and clean up resources.
    ///
    /// Sends a kill signal to the child process, waits for the reader thread
    /// to finish, and emits an `Exited` event with the process exit code.
    pub fn shutdown(&mut self) {
        self.running.store(false, Ordering::Release);

        if let Some(mut child) = self.child.take() {
            if let Err(e) = child.kill() {
                tracing::warn!("Failed to kill supervised process: {}", e);
            }

            // Wait for reader thread with timeout to avoid indefinite blocking
            let mut timed_out = true;
            if let Some(handle) = self.reader_handle.take() {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while std::time::Instant::now() < deadline {
                    if handle.is_finished() {
                        let _ = handle.join();
                        timed_out = false;
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                if timed_out {
                    tracing::warn!("Reader thread did not terminate within timeout");
                }
            }

            // Retrieve exit code via non-blocking try_wait
            let exit_code = match child.try_wait() {
                Ok(Some(status)) => status.exit_code() as i32,
                Ok(None) => -1,
                Err(e) => {
                    tracing::warn!("Failed to retrieve exit status: {}", e);
                    -1
                }
            };

            if let Some(tx) = self.event_tx.take() {
                let _ = tx.send(SupervisorEvent::Exited(exit_code));
            }
        }

        self.writer = None;
        self.master = None;
    }
}

impl Drop for ClaudeSupervisor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Strip ANSI escape sequences from text.
///
/// Returns the original text if stripping produces invalid UTF-8.
fn strip_ansi(text: &str) -> String {
    let bytes = text.as_bytes();
    let stripped = strip_ansi_escapes::strip(bytes);
    String::from_utf8(stripped).unwrap_or_else(|_| text.to_string())
}

/// Detect semantic events from cleaned output text.
///
/// Uses heuristic string matching — patterns may need adjustment for
/// different CLI tool versions or localization.
fn detect_event(text: &str) -> SupervisorEvent {
    // Approval request detection
    if text.contains("Do you want to run") || text.contains("Allow this command") {
        return SupervisorEvent::ApprovalRequest(text.to_string());
    }

    // Context overflow detection
    if text.contains("Context window") && text.contains("full") {
        return SupervisorEvent::ContextOverflow;
    }

    // Error detection
    if text.starts_with("Error:") || text.contains("error:") {
        return SupervisorEvent::Error(text.to_string());
    }

    // Default: regular output
    SupervisorEvent::Output(text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supervisor_creation() {
        let config = SupervisorConfig::new("/tmp");
        let supervisor = ClaudeSupervisor::new(config);
        assert!(!supervisor.is_running());
    }

    #[test]
    fn test_strip_ansi() {
        let input = "\x1b[31mRed text\x1b[0m";
        let output = strip_ansi(input);
        assert_eq!(output, "Red text");
    }

    #[test]
    fn test_strip_ansi_plain() {
        let input = "Plain text";
        let output = strip_ansi(input);
        assert_eq!(output, "Plain text");
    }

    #[test]
    fn test_detect_approval_request() {
        let text = "Do you want to run this command?";
        let event = detect_event(text);
        assert!(matches!(event, SupervisorEvent::ApprovalRequest(_)));
    }

    #[test]
    fn test_detect_context_overflow() {
        let text = "Context window is full. Consider using /compact.";
        let event = detect_event(text);
        assert!(matches!(event, SupervisorEvent::ContextOverflow));
    }

    #[test]
    fn test_detect_error() {
        let text = "Error: Command not found";
        let event = detect_event(text);
        assert!(matches!(event, SupervisorEvent::Error(_)));
    }

    #[test]
    fn test_detect_output() {
        let text = "Hello, world!";
        let event = detect_event(text);
        assert!(matches!(event, SupervisorEvent::Output(_)));
    }

    #[test]
    fn test_secret_masking_in_output() {
        let masker = crate::exec::SecretMasker::new();
        let input = "API_KEY=sk-abcdefghijklmnopqrstuvwxyz12345678901234";
        let masked = masker.mask(input);
        assert!(masked.contains("***REDACTED***"));
        assert!(!masked.contains("abcdefghijklmnopqrstuvwxyz"));
    }

    #[test]
    fn test_args_validation_rejects_injection() {
        let result = SupervisorConfig::new("/tmp").with_args(vec!["foo; rm -rf /".to_string()]);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("forbidden character"));
    }

    #[test]
    fn test_args_validation_accepts_safe_args() {
        let result =
            SupervisorConfig::new("/tmp").with_args(vec!["--help".to_string(), "file.txt".to_string()]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_custom_secret_pattern() {
        let config = SupervisorConfig::new("/tmp")
            .with_secret_pattern(r"SECRET_\d+", "SECRET_REDACTED");
        let supervisor = ClaudeSupervisor::new(config);
        let masked = supervisor.masker.mask("Token: SECRET_12345");
        assert!(masked.contains("SECRET_REDACTED"));
    }
}
