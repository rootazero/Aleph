//! Optional lifecycle control for a one-shot shell command. The platform
//! driver owns the child and records proof; the registry never owns a PID.

#[cfg(target_os = "linux")]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;

use tokio_util::sync::CancellationToken;

use super::SandboxError;

#[cfg(target_os = "linux")]
use super::shell_input::{ShellEffectDenied, ShellEffectResult, ShellInputAction, ShellPipe};

#[derive(Default)]
struct ShellFence {
    #[cfg(target_os = "linux")]
    input: Option<ShellPipe>,
}

impl ShellFence {
    fn discard_input(&mut self) {
        #[cfg(target_os = "linux")]
        drop(self.input.take());
    }
}

const PREPARING: u8 = 0;
const SPAWNED: u8 = 1;
const RUNNING: u8 = 2;
const REAPED: u8 = 3;
const CANCELLED: u8 = 4;

#[derive(Default)]
pub struct ShellLifecycle {
    cancel: CancellationToken,
    spawn_fence: Mutex<ShellFence>,
    phase: AtomicU8,
    #[cfg(target_os = "linux")]
    explicit_eof: AtomicBool,
}

impl ShellLifecycle {
    pub(crate) fn request_cancel(&self) {
        let mut fence = self
            .spawn_fence
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        self.cancel.cancel();
        fence.discard_input();
    }

    pub(crate) fn cancellation_requested(&self) -> bool {
        self.cancel.is_cancelled()
    }

    pub(crate) async fn cancelled(&self) {
        self.cancel.cancelled().await;
    }

    /// Order cancellation against the actual, synchronous OS spawn. Admission,
    /// native setup after spawn, and reaping must stay outside this fence.
    pub(crate) fn spawn<T>(
        &self,
        spawn: impl FnOnce() -> Result<T, SandboxError>,
    ) -> Result<T, SandboxError> {
        let _fence = self
            .spawn_fence
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        if self.cancellation_requested() {
            return Err(self.cancel_before_spawn());
        }
        let child = spawn()?;
        self.phase.store(SPAWNED, Ordering::Release);
        Ok(child)
    }

    /// Called when cancellation prevented launch, including during admission.
    pub(crate) fn cancel_before_spawn(&self) -> SandboxError {
        self.phase.store(CANCELLED, Ordering::Release);
        SandboxError::ExecutionFailed("shell launch cancelled".into())
    }

    pub(crate) fn ready(&self) {
        self.phase.store(RUNNING, Ordering::Release);
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn ready_with_input(&self, writer: std::fs::File) -> std::io::Result<()> {
        let input = ShellPipe::new(writer)?;
        let mut fence = self
            .spawn_fence
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        if self.cancellation_requested() || self.phase.load(Ordering::Acquire) != SPAWNED {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "shell not starting",
            ));
        }
        fence.input = Some(input);
        self.ready();
        Ok(())
    }

    #[cfg(target_os = "linux")]
    pub(super) fn input_snapshot(&self) -> Result<(Vec<u8>, bool), ShellEffectDenied> {
        let fence = self
            .spawn_fence
            .try_lock()
            .map_err(|_| ShellEffectDenied::Busy)?;
        if self.cancellation_requested() || self.phase.load(Ordering::Acquire) != RUNNING {
            return Err(ShellEffectDenied::Stale);
        }
        let input = fence.input.as_ref().ok_or(ShellEffectDenied::NotWritable)?;
        Ok((input.prefix().to_vec(), input.is_closed()))
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn input_progress_for_test(&self) -> Result<(usize, bool), ShellEffectDenied> {
        self.input_snapshot()
            .map(|(prefix, closed)| (prefix.len(), closed))
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn has_explicit_eof(&self) -> bool {
        self.explicit_eof.load(Ordering::Acquire)
    }

    #[cfg(target_os = "linux")]
    pub(super) fn commit_input(
        &self,
        prefix_bytes: usize,
        deadline: std::time::Instant,
        account_expiry: Option<std::time::SystemTime>,
        attempt_cancel: &CancellationToken,
        action: ShellInputAction,
    ) -> Result<ShellEffectResult, ShellEffectDenied> {
        let mut fence = self
            .spawn_fence
            .try_lock()
            .map_err(|_| ShellEffectDenied::Busy)?;
        if attempt_cancel.is_cancelled()
            || std::time::Instant::now() >= deadline
            || account_expiry.is_some_and(|expiry| std::time::SystemTime::now() >= expiry)
        {
            return Err(ShellEffectDenied::Expired);
        }
        if matches!(action, ShellInputAction::ObserveClosed) && self.has_explicit_eof() {
            return Ok(ShellEffectResult::AlreadyClosed);
        }
        if self.cancellation_requested() || self.phase.load(Ordering::Acquire) != RUNNING {
            return Err(ShellEffectDenied::Stale);
        }
        let input = fence.input.as_mut().ok_or(ShellEffectDenied::NotWritable)?;
        if input.prefix().len() != prefix_bytes {
            return Err(ShellEffectDenied::Stale);
        }
        let effect = input.apply(action)?;
        if effect == ShellEffectResult::Closed {
            self.explicit_eof.store(true, Ordering::Release);
        }
        if effect == ShellEffectResult::Ambiguous {
            self.cancel.cancel();
            fence.discard_input();
        }
        Ok(effect)
    }

    /// Retire input when the native wait ends, without yet publishing a
    /// terminal state. Strict jobs still owe proof that the group is empty.
    #[cfg(target_os = "linux")]
    pub(crate) fn retire_input(&self) {
        self.spawn_fence
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .discard_input();
    }

    /// Called only after the driver has observed exit and completed its
    /// teardown contract (direct child or, for the strict profile, the group).
    pub(crate) fn reaped(&self, cancelled: bool) {
        let mut fence = self
            .spawn_fence
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        fence.discard_input();
        let phase = if cancelled { CANCELLED } else { REAPED };
        self.phase.store(phase, Ordering::Release);
    }

    pub(crate) fn started(&self) -> bool {
        matches!(self.phase.load(Ordering::Acquire), RUNNING | REAPED)
    }

    pub(crate) fn settled(&self) -> bool {
        matches!(
            self.phase.load(Ordering::Acquire),
            PREPARING | REAPED | CANCELLED
        )
    }

    pub(crate) fn stopped(&self) -> bool {
        self.phase.load(Ordering::Acquire) == CANCELLED
    }
}
