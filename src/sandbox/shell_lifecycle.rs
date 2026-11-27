//! Optional lifecycle control for a one-shot shell command. The platform
//! driver owns the child and records proof; the registry never owns a PID.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;

use tokio_util::sync::CancellationToken;

use super::SandboxError;

const PREPARING: u8 = 0;
const SPAWNED: u8 = 1;
const RUNNING: u8 = 2;
const REAPED: u8 = 3;
const CANCELLED: u8 = 4;

#[derive(Default)]
pub struct ShellLifecycle {
    cancel: CancellationToken,
    spawn_fence: Mutex<()>,
    phase: AtomicU8,
}

impl ShellLifecycle {
    pub(crate) fn request_cancel(&self) {
        let _fence = self
            .spawn_fence
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        self.cancel.cancel();
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

    /// Called only after the driver has observed a direct-child exit.
    pub(crate) fn reaped(&self, cancelled: bool) {
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
