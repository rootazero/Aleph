//! PTY-based supervisor implementation.

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize as PortablePtySize};
use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::exec::SecretMasker;
use crate::supervisor::types::{SupervisorConfig, SupervisorError, SupervisorEvent};

/// PTY-based supervisor for controlling Claude Code and similar CLI tools.
///
/// # Example
///
/// ```rust,no_run
/// use aethecore::supervisor::{ClaudeSupervisor, SupervisorConfig};
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
    running: Arc<AtomicBool>,
    masker: SecretMasker,
}

impl ClaudeSupervisor {
    /// Create a new supervisor with the given configuration.
    pub fn new(config: SupervisorConfig) -> Self {
        Self {
            config,
            master: None,
            writer: None,
            running: Arc::new(AtomicBool::new(false)),
            masker: SecretMasker::new(),
        }
    }

    /// Check if the supervised process is currently running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
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

        // Spawn process
        let _child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| SupervisorError::SpawnFailed(e.to_string()))?;

        // Get reader and writer
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| SupervisorError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| SupervisorError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;

        self.master = Some(pair.master);
        self.writer = Some(writer);
        self.running.store(true, Ordering::SeqCst);

        // Create event channel
        let (tx, rx) = mpsc::unbounded_channel();
        let running = self.running.clone();
        let masker = self.masker.clone();

        // Spawn reader thread
        std::thread::spawn(move || {
            let buf_reader = BufReader::new(reader);
            for line in buf_reader.lines() {
                match line {
                    Ok(text) => {
                        // Strip ANSI escape sequences
                        let clean = strip_ansi(&text);
                        // Mask secrets in output
                        let safe = masker.mask(&clean);

                        // Detect semantic events
                        let event = detect_event(&safe);
                        if tx.send(event).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            running.store(false, Ordering::SeqCst);
            let _ = tx.send(SupervisorEvent::Exited(0));
        });

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
        self.write(&format!("{}\n", input))
    }
}

/// Strip ANSI escape sequences from text.
fn strip_ansi(text: &str) -> String {
    let bytes = text.as_bytes();
    let stripped = strip_ansi_escapes::strip(bytes);
    String::from_utf8_lossy(&stripped).to_string()
}

/// Detect semantic events from cleaned output text.
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
        // SecretMasker should be used in supervisor
        let masker = crate::exec::SecretMasker::new();
        let input = "API_KEY=sk-abcdefghijklmnopqrstuvwxyz12345678901234";
        let masked = masker.mask(input);
        assert!(masked.contains("***REDACTED***"));
        assert!(!masked.contains("abcdefghijklmnopqrstuvwxyz"));
    }
}
