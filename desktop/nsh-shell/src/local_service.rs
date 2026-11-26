//! The bundled local server, started in the background so the window can
//! show progress, and a failure the user can retry instead of an exit.

use std::sync::{Arc, Mutex};
use tokio::sync::watch;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ServerPhase {
    Starting,
    Ready,
    Failed(String),
}

/// Brings the local server up; blocks until it is ready or refused.
pub(crate) type Launcher = Box<dyn Fn() -> Result<(), String> + Send + Sync>;

#[derive(Clone)]
pub(crate) struct LocalService {
    launcher: Arc<Launcher>,
    phase: Arc<watch::Sender<ServerPhase>>,
    /// Whether an attempt is in flight. Held while the phase changes, so an
    /// attempt that settles cannot overwrite the start of a newer one.
    running: Arc<Mutex<bool>>,
}

impl LocalService {
    pub(crate) fn new(launcher: Launcher) -> Self {
        Self {
            launcher: Arc::new(launcher),
            phase: Arc::new(watch::Sender::new(ServerPhase::Starting)),
            running: Arc::new(Mutex::new(false)),
        }
    }

    /// The `aleph-server` next to this executable, as the packaged app
    /// ships it; a compatible server already listening is used as it is.
    pub(crate) fn bundled() -> Self {
        Self::new(Box::new(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "could not initialize the local service runtime".to_owned())?
                .block_on(aleph_desktop_host::local_server::ensure_compatible_ready())
        }))
    }

    /// Starts an attempt in the background, unless one is in flight.
    pub(crate) fn start(&self) {
        let mut running = self
            .running
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if *running {
            return;
        }
        *running = true;
        self.phase.send_replace(ServerPhase::Starting);
        let service = self.clone();
        let attempt = std::thread::Builder::new()
            .name("aleph-local-service".into())
            .spawn(move || {
                let outcome = (service.launcher)();
                service.settle(match outcome {
                    Ok(()) => ServerPhase::Ready,
                    Err(error) => ServerPhase::Failed(error),
                });
            });
        if attempt.is_err() {
            *running = false;
            self.phase.send_replace(ServerPhase::Failed(
                "could not start the local service thread".into(),
            ));
        }
    }

    fn settle(&self, phase: ServerPhase) {
        let mut running = self
            .running
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        *running = false;
        self.phase.send_replace(phase);
    }

    pub(crate) fn phase(&self) -> ServerPhase {
        self.phase.borrow().clone()
    }

    /// Every phase change from now on, starting with the current phase.
    pub(crate) fn subscribe(&self) -> watch::Receiver<ServerPhase> {
        self.phase.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::{LocalService, ServerPhase};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Arc, Mutex};
    use std::time::{Duration, Instant};

    /// Waits until the attempt in flight settles.
    fn settled(service: &LocalService) -> ServerPhase {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let phase = service.phase();
            if phase != ServerPhase::Starting || Instant::now() > deadline {
                return phase;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// A launcher that blocks until the test releases it, counting calls.
    fn gated(result: Result<(), String>) -> (LocalService, mpsc::Sender<()>, Arc<AtomicUsize>) {
        let (release, gate) = mpsc::channel::<()>();
        let gate = Mutex::new(gate);
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let service = LocalService::new(Box::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            let _ = gate.lock().unwrap().recv();
            result.clone()
        }));
        (service, release, calls)
    }

    #[test]
    fn phase_moves_starting_to_ready() {
        let (service, release, _) = gated(Ok(()));
        service.start();
        assert_eq!(service.phase(), ServerPhase::Starting);
        release.send(()).unwrap();
        assert_eq!(settled(&service), ServerPhase::Ready);
    }

    #[test]
    fn failure_is_reported_not_exited() {
        let service = LocalService::new(Box::new(|| Err("port 18790 busy".into())));
        service.start();
        assert_eq!(
            settled(&service),
            ServerPhase::Failed("port 18790 busy".into())
        );
    }

    #[test]
    fn retry_calls_launcher_again() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let service = LocalService::new(Box::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            Err("port 18790 busy".into())
        }));
        service.start();
        assert!(matches!(settled(&service), ServerPhase::Failed(_)));
        service.start();
        assert!(matches!(settled(&service), ServerPhase::Failed(_)));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn concurrent_start_runs_launcher_once() {
        let (service, release, calls) = gated(Ok(()));
        service.start();
        service.start();
        service.clone().start();
        release.send(()).unwrap();
        assert_eq!(settled(&service), ServerPhase::Ready);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
