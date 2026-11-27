//! Single-use authority handoff for the fixed Linux JSONL reader.

use std::fs::File;
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::FileTypeExt;
use std::time::Instant;

use crate::approval::operator_requester::OperatorApprovalRequester;
use crate::sync_primitives::Arc;
use tokio_util::sync::CancellationToken;

use super::exec_approval::{gate::record_gate_decision, ApprovalAction, ApprovalOutcome};
use super::shell_lifecycle::ShellLifecycle;
use super::shell_policy::ShellPolicy;
use super::SandboxCommand;

pub(crate) const MAX_FRAME: usize = 4096;
pub(crate) const MAX_PREFIX: usize = 16 * 1024;
pub(crate) const JSONL_PROGRAM: &str = "/usr/bin/jq";
pub(crate) const JSONL_RECORDS_PATH: &str = "/records";
pub(crate) const JSONL_ARGS: [&str; 5] = [
    "-M",
    "-c",
    "--unbuffered",
    r#"{kind:type, count:(if type == "array" or type == "object" then length else null end)}"#,
    JSONL_RECORDS_PATH,
];

pub(crate) enum ShellInputAction {
    Write(JsonlFrame),
    Close,
    ObserveClosed,
}

/// Only complete JSON values enter the fixed consumer, never arbitrary bytes.
/// Limit serialization itself instead of allocating an unbounded copy first.
pub(crate) struct JsonlFrame(Vec<u8>);

impl JsonlFrame {
    pub(crate) fn from_record(record: &serde_json::Value) -> Result<Self, ShellEffectDenied> {
        struct FrameBuffer(Vec<u8>);

        impl Write for FrameBuffer {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if bytes.len() >= MAX_FRAME - self.0.len() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "JSONL frame too large",
                    ));
                }
                self.0.extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let mut buffer = FrameBuffer(Vec::new());
        serde_json::to_writer(&mut buffer, record).map_err(|_| ShellEffectDenied::TooLarge)?;
        buffer.0.push(b'\n');
        Ok(Self(buffer.0))
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ShellEffectResult {
    Written(usize),
    Closed,
    AlreadyClosed,
    Ambiguous,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ShellEffectDenied {
    Busy,
    Stale,
    Expired,
    NotWritable,
    TooLarge,
    Backpressure,
    ReaderGone,
    UnverifiedPolicy,
    PolicyBlocked,
    ApprovalRequired,
}

/// This sealed operation has no arbitrary callback and holds the exact live
/// controller in memory. It is consumed once; delivery is never retried.
pub(crate) struct ShellPipeAttempt {
    job: Arc<ShellLifecycle>,
    prefix_bytes: usize,
    deadline: Instant,
    account_expiry: Option<std::time::SystemTime>,
    cancel: CancellationToken,
    action: ShellInputAction,
}

impl ShellPipeAttempt {
    pub(crate) async fn prepare(
        job: Arc<ShellLifecycle>,
        job_id: u64,
        policy: &ShellPolicy,
        launch: &SandboxCommand,
        approval: &OperatorApprovalRequester,
        deadline: Instant,
        cancel: CancellationToken,
        action: ShellInputAction,
    ) -> Result<Self, ShellEffectDenied> {
        if cancel.is_cancelled() || Instant::now() >= deadline {
            return Err(ShellEffectDenied::Expired);
        }
        let (prefix, closed) = job.input_snapshot()?;
        let action = match action {
            ShellInputAction::Write(_) if closed => return Err(ShellEffectDenied::NotWritable),
            ShellInputAction::Write(ref frame) => {
                let warnings = policy.check_frame(launch, &prefix, frame)?;
                if warnings.operator || !warnings.builtin.is_empty() {
                    tracing::warn!(%job_id, "shell input matched an advisory rule");
                }
                action
            }
            ShellInputAction::Close if closed => ShellInputAction::ObserveClosed,
            _ => action,
        };
        let approval_action = match &action {
            ShellInputAction::Write(frame) => Some(ApprovalAction::for_shell_input(
                job_id,
                Some(frame.bytes().len()),
            )),
            ShellInputAction::Close => Some(ApprovalAction::for_shell_input(job_id, None)),
            ShellInputAction::ObserveClosed => None,
        };
        let deadline = if let Some(approval_action) = approval_action {
            // Final delivery cannot outlive the operator's approval window.
            let deadline = deadline.min(Instant::now() + OperatorApprovalRequester::ONCE_WINDOW);
            // Require a human's literal AllowOnce, not an automatic or widened grant.
            let decision = approval
                .request_operator_once(&approval_action, &cancel, deadline.into())
                .await;
            if decision.outcome != ApprovalOutcome::Approved {
                return Err(if cancel.is_cancelled() || Instant::now() >= deadline {
                    ShellEffectDenied::Expired
                } else {
                    ShellEffectDenied::ApprovalRequired
                });
            }
            record_gate_decision(&approval_action, &decision).await;
            deadline
        } else {
            deadline
        };
        if cancel.is_cancelled() || Instant::now() >= deadline {
            return Err(ShellEffectDenied::Expired);
        }
        Ok(Self {
            job,
            prefix_bytes: prefix.len(),
            deadline,
            account_expiry: None,
            cancel,
            action,
        })
    }

    pub(crate) fn with_account_expiry(mut self, expiry: std::time::SystemTime) -> Self {
        self.account_expiry = Some(
            self.account_expiry
                .map_or(expiry, |current| current.min(expiry)),
        );
        self
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(
        job: Arc<ShellLifecycle>,
        prefix_bytes: usize,
        deadline: Instant,
        action: ShellInputAction,
    ) -> Self {
        Self {
            job,
            prefix_bytes,
            deadline,
            account_expiry: None,
            cancel: CancellationToken::new(),
            action,
        }
    }

    pub(crate) fn consume(self) -> Result<ShellEffectResult, ShellEffectDenied> {
        self.job.commit_input(
            self.prefix_bytes,
            self.deadline,
            self.account_expiry,
            &self.cancel,
            self.action,
        )
    }
}

#[cfg(test)]
pub(crate) fn nonblocking_test_pipe() -> (File, File) {
    use std::os::fd::FromRawFd;

    let mut descriptors = [-1; 2];
    // SAFETY: pipe2 initializes two uniquely owned descriptors on success.
    assert_eq!(
        unsafe { libc::pipe2(descriptors.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) },
        0
    );
    unsafe {
        (
            File::from_raw_fd(descriptors[0]),
            File::from_raw_fd(descriptors[1]),
        )
    }
}

pub(super) struct ShellPipe {
    writer: Option<File>,
    atomic_limit: usize,
    prefix: Vec<u8>,
}

impl ShellPipe {
    /// The driver must supply its sole writer for a pipe whose only reader is
    /// the supervised consumer. Duplicated writer/read handles are not allowed.
    pub(super) fn new(writer: File) -> io::Result<Self> {
        let fd = writer.as_raw_fd();
        // SAFETY: the owned descriptor stays open throughout these queries.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        let native_limit = unsafe { libc::fpathconf(fd, libc::_PC_PIPE_BUF) };
        if flags < 0
            || flags & libc::O_ACCMODE != libc::O_WRONLY
            || flags & libc::O_NONBLOCK == 0
            || native_limit <= 0
            || !writer.metadata()?.file_type().is_fifo()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected a nonblocking pipe writer",
            ));
        }
        Ok(Self {
            writer: Some(writer),
            atomic_limit: MAX_FRAME.min(native_limit as usize),
            prefix: Vec::with_capacity(MAX_PREFIX),
        })
    }

    pub(super) fn prefix(&self) -> &[u8] {
        &self.prefix
    }

    pub(super) fn is_closed(&self) -> bool {
        self.writer.is_none()
    }

    pub(super) fn apply(
        &mut self,
        action: ShellInputAction,
    ) -> Result<ShellEffectResult, ShellEffectDenied> {
        let frame = match action {
            ShellInputAction::ObserveClosed => {
                return if self.is_closed() {
                    Ok(ShellEffectResult::AlreadyClosed)
                } else {
                    Err(ShellEffectDenied::Stale)
                };
            }
            ShellInputAction::Close => {
                return Ok(if self.writer.take().is_some() {
                    ShellEffectResult::Closed
                } else {
                    ShellEffectResult::AlreadyClosed
                });
            }
            ShellInputAction::Write(frame) => frame,
        };
        let frame = frame.bytes();
        if frame.len() > self.atomic_limit || frame.len() > MAX_PREFIX - self.prefix.len() {
            return Err(ShellEffectDenied::TooLarge);
        }
        let writer = self.writer.as_mut().ok_or(ShellEffectDenied::NotWritable)?;
        match writer.write(frame) {
            Ok(n) if n == frame.len() => {
                self.prefix.extend_from_slice(frame);
                Ok(ShellEffectResult::Written(n))
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                Err(ShellEffectDenied::Backpressure)
            }
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {
                Err(ShellEffectDenied::ReaderGone)
            }
            _ => {
                // Do not add an uncertain frame to the authorized prefix or
                // keep serving a stream that may now contain a partial record.
                self.writer.take();
                Ok(ShellEffectResult::Ambiguous)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::time::Duration;

    fn attempt(
        job: &Arc<ShellLifecycle>,
        prefix: usize,
        action: ShellInputAction,
    ) -> ShellPipeAttempt {
        ShellPipeAttempt::new_for_test(
            job.clone(),
            prefix,
            Instant::now() + Duration::from_secs(10),
            action,
        )
    }

    #[test]
    fn shell_jsonl_frame_is_bounded_and_escapes_internal_newlines() {
        let record = serde_json::Value::String("é".repeat((MAX_FRAME - 4) / 2) + "a");
        assert_eq!(
            JsonlFrame::from_record(&record).unwrap().bytes().len(),
            MAX_FRAME
        );
        let oversized = serde_json::Value::String("é".repeat((MAX_FRAME - 2) / 2));
        assert!(matches!(
            JsonlFrame::from_record(&oversized),
            Err(ShellEffectDenied::TooLarge)
        ));
        let multiline = JsonlFrame::from_record(&serde_json::json!("one\ntwo")).unwrap();
        assert_eq!(multiline.bytes(), b"\"one\\ntwo\"\n");
    }

    #[test]
    fn shell_effect_refuses_closed_reader_or_full_pipe() {
        let (mut reader, mut writer) = nonblocking_test_pipe();
        let fill = [b'x'; MAX_FRAME];
        while writer.write(&fill).is_ok() {}

        let job = Arc::new(ShellLifecycle::default());
        job.spawn(|| Ok(())).unwrap();
        job.ready_with_input(writer).unwrap();
        let write = || {
            attempt(
                &job,
                0,
                ShellInputAction::Write(JsonlFrame::from_record(&serde_json::Value::Null).unwrap()),
            )
            .consume()
        };
        assert_eq!(write(), Err(ShellEffectDenied::Backpressure));
        assert!(job.input_snapshot().unwrap().0.is_empty());
        let mut drained = [0; MAX_FRAME];
        reader.read_exact(&mut drained).unwrap();
        assert_eq!(drained, fill);
        assert_eq!(write(), Ok(ShellEffectResult::Written(5)));
        assert_eq!(job.input_snapshot().unwrap().0, b"null\n");
        assert_eq!(write(), Err(ShellEffectDenied::Stale));

        // The job has not reported reaping. The kernel still refuses the
        // write when the sole reader disappears.
        drop(reader);
        assert_eq!(
            attempt(
                &job,
                5,
                ShellInputAction::Write(
                    JsonlFrame::from_record(&serde_json::Value::Bool(true)).unwrap()
                )
            )
            .consume(),
            Err(ShellEffectDenied::ReaderGone)
        );
        assert_eq!(job.input_snapshot().unwrap().0, b"null\n");
    }

    #[test]
    fn shell_effect_orders_deadline_eof_and_stop() {
        let (mut reader, writer) = nonblocking_test_pipe();
        let job = Arc::new(ShellLifecycle::default());
        job.spawn(|| Ok(())).unwrap();
        job.ready_with_input(writer).unwrap();
        let expired =
            ShellPipeAttempt::new_for_test(job.clone(), 0, Instant::now(), ShellInputAction::Close);
        assert_eq!(expired.consume(), Err(ShellEffectDenied::Expired));
        assert_eq!(
            reader.read(&mut [0]).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(
            attempt(&job, 0, ShellInputAction::Close).consume(),
            Ok(ShellEffectResult::Closed)
        );
        assert_eq!(reader.read(&mut [0]).unwrap(), 0);
        assert_eq!(
            attempt(&job, 0, ShellInputAction::Close).consume(),
            Ok(ShellEffectResult::AlreadyClosed)
        );
        job.request_cancel();
        assert_eq!(
            attempt(&job, 0, ShellInputAction::Close).consume(),
            Err(ShellEffectDenied::Stale)
        );
    }
}
