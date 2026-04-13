//! Tool execution context with cascade policy and progress reporting.
//!
//! Provides per-tool cancellation, progress streaming, and cascade classification
//! for the streaming tool bridge.

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Determines how a tool's failure or completion affects sibling tools
/// running in the same parallel batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CascadePolicy {
    /// Side-effecting tools: failure aborts all siblings in the batch.
    AbortSiblings,
    /// Read-only / isolated tools: failure does not affect siblings.
    Isolated,
}

impl CascadePolicy {
    /// Classify a tool by name into its cascade policy.
    pub fn classify(tool_name: &str) -> Self {
        match tool_name {
            "Bash" | "Write" | "Edit" | "NotebookEdit" => Self::AbortSiblings,
            _ => Self::Isolated,
        }
    }
}

/// Progress events emitted by a running tool.
#[derive(Debug, Clone)]
pub enum ToolProgress {
    /// Human-readable status update (e.g. "compiling...", "downloading 3/10").
    Status { tool_id: String, message: String },
    /// Incremental output chunk (e.g. streaming stdout).
    PartialOutput { tool_id: String, chunk: String },
}

/// Per-tool execution context carrying cancellation, progress, and policy.
pub struct ToolExecutionContext {
    /// Child token derived from the batch-level token.
    pub cancel: CancellationToken,
    /// Channel for sending progress events to the bridge.
    pub progress_tx: mpsc::Sender<ToolProgress>,
    /// How this tool's outcome cascades to siblings.
    pub cascade_policy: CascadePolicy,
}

impl ToolExecutionContext {
    /// Create a new context for a specific tool invocation.
    ///
    /// `batch_cancel` is the parent token shared by all tools in the batch;
    /// a child token is derived so individual tools can be cancelled independently.
    pub fn new(
        batch_cancel: &CancellationToken,
        progress_tx: mpsc::Sender<ToolProgress>,
        tool_name: &str,
    ) -> Self {
        Self {
            cancel: batch_cancel.child_token(),
            progress_tx,
            cascade_policy: CascadePolicy::classify(tool_name),
        }
    }

    /// Best-effort send of a progress event (drops if the channel is full).
    pub fn send_progress(&self, progress: ToolProgress) {
        let _ = self.progress_tx.try_send(progress);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_bash_as_abort_siblings() {
        assert_eq!(
            CascadePolicy::classify("Bash"),
            CascadePolicy::AbortSiblings
        );
    }

    #[test]
    fn classify_write_as_abort_siblings() {
        assert_eq!(
            CascadePolicy::classify("Write"),
            CascadePolicy::AbortSiblings
        );
    }

    #[test]
    fn classify_edit_as_abort_siblings() {
        assert_eq!(
            CascadePolicy::classify("Edit"),
            CascadePolicy::AbortSiblings
        );
    }

    #[test]
    fn classify_read_as_isolated() {
        assert_eq!(CascadePolicy::classify("Read"), CascadePolicy::Isolated);
    }

    #[test]
    fn classify_unknown_as_isolated() {
        assert_eq!(
            CascadePolicy::classify("SomeCustomTool"),
            CascadePolicy::Isolated
        );
    }

    #[tokio::test]
    async fn progress_send_receive() {
        let (tx, mut rx) = mpsc::channel(16);
        let batch_cancel = CancellationToken::new();
        let ctx = ToolExecutionContext::new(&batch_cancel, tx, "Bash");

        assert_eq!(ctx.cascade_policy, CascadePolicy::AbortSiblings);

        ctx.send_progress(ToolProgress::Status {
            tool_id: "t1".into(),
            message: "running".into(),
        });

        ctx.send_progress(ToolProgress::PartialOutput {
            tool_id: "t1".into(),
            chunk: "hello world".into(),
        });

        let first = rx.recv().await.expect("should receive first progress");
        match first {
            ToolProgress::Status { tool_id, message } => {
                assert_eq!(tool_id, "t1");
                assert_eq!(message, "running");
            }
            _ => panic!("expected Status variant"),
        }

        let second = rx.recv().await.expect("should receive second progress");
        match second {
            ToolProgress::PartialOutput { tool_id, chunk } => {
                assert_eq!(tool_id, "t1");
                assert_eq!(chunk, "hello world");
            }
            _ => panic!("expected PartialOutput variant"),
        }
    }
}
