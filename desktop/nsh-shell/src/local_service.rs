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

/// Stops the local server; blocks until its port is closed.
pub(crate) type Stopper = Box<dyn Fn() + Send + Sync>;

#[derive(Clone)]
pub(crate) struct LocalService {
    launcher: Arc<Launcher>,
    stopper: Arc<Stopper>,
    phase: Arc<watch::Sender<ServerPhase>>,
    /// Whether an attempt is in flight. Held while the phase changes, so an
    /// attempt that settles cannot overwrite the start of a newer one.
    running: Arc<Mutex<bool>>,
}

impl LocalService {
    pub(crate) fn new(launcher: Launcher, stopper: Stopper) -> Self {
        Self {
            launcher: Arc::new(launcher),
            stopper: Arc::new(stopper),
            phase: Arc::new(watch::Sender::new(ServerPhase::Starting)),
            running: Arc::new(Mutex::new(false)),
        }
    }

    /// The `aleph-server` next to this executable, as the packaged app
    /// ships it; a compatible server already listening is used as it is.
    pub(crate) fn bundled() -> Self {
        Self::new(
            Box::new(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|_| "could not initialize the local service runtime".to_owned())?
                    .block_on(aleph_desktop_host::local_server::ensure_compatible_ready())
            }),
            Box::new(stop_bundled),
        )
    }

    /// Stops the local server and waits for its port to close; blocks, so
    /// call it off the engine thread. The next [`Self::start`] brings it up.
    pub(crate) fn stop(&self) {
        (self.stopper)();
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

/// `aleph-server stop` for the server next to this executable, then waits
/// for the port to close so a later start does not find the old server.
fn stop_bundled() {
    use aleph_desktop_host::local_server::{resolve_daemon_binary, wait_until_port_closed};
    let Some(server) = resolve_daemon_binary() else {
        eprintln!("cannot stop the local server: aleph-server not found");
        return;
    };
    match std::process::Command::new(&server).arg("stop").output() {
        Ok(output) if output.status.success() => {}
        Ok(output) => eprintln!(
            "aleph-server stop exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Err(error) => eprintln!("aleph-server stop failed: {error}"),
    }
    match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime.block_on(wait_until_port_closed()),
        Err(error) => eprintln!("could not wait for the local server to stop: {error}"),
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
        let service = LocalService::new(
            Box::new(move || {
                counted.fetch_add(1, Ordering::SeqCst);
                let _ = gate.lock().unwrap().recv();
                result.clone()
            }),
            Box::new(|| {}),
        );
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
        let service =
            LocalService::new(Box::new(|| Err("port 18790 busy".into())), Box::new(|| {}));
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
        let service = LocalService::new(
            Box::new(move || {
                counted.fetch_add(1, Ordering::SeqCst);
                Err("port 18790 busy".into())
            }),
            Box::new(|| {}),
        );
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

    #[test]
    fn stop_runs_the_stopper_and_start_brings_it_back() {
        let stops = Arc::new(AtomicUsize::new(0));
        let counted = stops.clone();
        let service = LocalService::new(
            Box::new(|| Ok(())),
            Box::new(move || {
                counted.fetch_add(1, Ordering::SeqCst);
            }),
        );
        service.start();
        assert_eq!(settled(&service), ServerPhase::Ready);
        service.stop();
        assert_eq!(stops.load(Ordering::SeqCst), 1);
        service.start();
        assert_eq!(settled(&service), ServerPhase::Ready);
    }
}
